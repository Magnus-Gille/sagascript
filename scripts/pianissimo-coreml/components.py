"""NeMo wrappers and the fixed-shape local-attention export adapter."""

from __future__ import annotations

from typing import Tuple

import torch


class DenseLocalAttention(torch.nn.Module):
    """Export NeMo local relative attention as dense fixed-shape attention.

    NeMo's ``RelPositionMultiHeadAttentionLongformer`` computes a sliding
    window with overlap chunks.  CoreML conversion is considerably more
    reliable when the fixed window is expressed as ordinary dense matmuls.
    The relative-position scores are exactly the same values as NeMo's
    ``matrix_bd``; the constant mask below is the same +/- context band.
    """

    def __init__(self, source: torch.nn.Module, frames: int) -> None:
        super().__init__()
        self.source = source
        self.frames = int(frames)
        width = int(source.att_context_size[0])
        right = int(source.att_context_size[1])
        if width != 256 or right != 256:
            raise ValueError(f"Expected [256, 256] local attention, got {source.att_context_size}")

        query = torch.arange(self.frames, dtype=torch.long).view(self.frames, 1)
        key = torch.arange(self.frames, dtype=torch.long).view(1, self.frames)
        relative = (key - query + width).clamp(0, width + right)
        band_mask = (torch.abs(key - query) > width).unsqueeze(0)
        self.register_buffer("relative_index", relative, persistent=False)
        self.register_buffer("band_mask", band_mask, persistent=False)
        self.register_buffer(
            "band_additive", band_mask.to(dtype=torch.float32) * -10000.0, persistent=False
        )
        self.register_buffer(
            "band_valid", (~band_mask).to(dtype=torch.float32), persistent=False
        )

    def forward(
        self,
        query: torch.Tensor,
        key: torch.Tensor,
        value: torch.Tensor,
        pad_mask: torch.Tensor,
        pos_emb: torch.Tensor,
        cache=None,
    ) -> torch.Tensor:
        if cache is not None:
            raise RuntimeError("DenseLocalAttention export does not support streaming cache")

        q, k, v = self.source.forward_qkv(query, key, value)
        q_time = q.transpose(1, 2)
        pos = self.source.linear_pos(pos_emb).view(
            pos_emb.size(0), -1, self.source.h, self.source.d_k
        ).transpose(1, 2)
        q_u = (q_time + self.source.pos_bias_u).transpose(1, 2)
        q_v = (q_time + self.source.pos_bias_v).transpose(1, 2)

        matrix_ac = torch.matmul(q_u, k.transpose(-2, -1))
        matrix_bd = torch.matmul(q_v, pos.transpose(-2, -1))
        indices = self.relative_index.unsqueeze(0).unsqueeze(0).expand(
            q.size(0), q.size(1), self.frames, self.frames
        )
        matrix_bd = torch.gather(matrix_bd, dim=-1, index=indices)
        scores = (matrix_ac + matrix_bd) / self.source.s_d_k

        # Conversion is for a fixed, full window.  The CoreML contract carries
        # mel_length for FluidAudio bookkeeping, but the exported fixed window
        # has no padding keys.  Materialize the band as arithmetic tensors:
        # CoreML handles this more faithfully than a dynamic masked_fill for
        # 30 s / 376-frame graphs.
        del pad_mask
        scores = scores + self.band_additive.to(dtype=scores.dtype)
        attn = torch.softmax(scores, dim=-1)
        attn = attn * self.band_valid.to(dtype=attn.dtype)
        p_attn = self.source.dropout(attn)
        x = torch.matmul(p_attn, v)
        x = x.transpose(1, 2).reshape(q.size(0), -1, self.source.h * self.source.d_k)
        return self.source.linear_out(x)


def patch_local_attention(encoder: torch.nn.Module, frames: int) -> None:
    """Replace every NeMo local attention module with the fixed dense form."""

    replaced = 0
    for layer in encoder.layers:
        if getattr(layer, "self_attention_model", None) != "rel_pos_local_attn":
            raise ValueError(
                "Pianissimo checkpoint is not using rel_pos_local_attn in every encoder layer"
            )
        layer.self_attn = DenseLocalAttention(layer.self_attn, frames)
        replaced += 1
    if replaced != 24:
        raise ValueError(f"Expected 24 patched attention layers, got {replaced}")


class PreprocessorWrapper(torch.nn.Module):
    def __init__(self, module: torch.nn.Module) -> None:
        super().__init__()
        self.module = module

    def forward(self, audio_signal: torch.Tensor, audio_length: torch.Tensor):
        return self.module(
            input_signal=audio_signal,
            length=audio_length.to(dtype=torch.long),
        )


class EncoderWrapper(torch.nn.Module):
    def __init__(self, module: torch.nn.Module) -> None:
        super().__init__()
        self.module = module

    def forward(self, mel: torch.Tensor, mel_length: torch.Tensor):
        return self.module(
            audio_signal=mel,
            length=mel_length.to(dtype=torch.long),
        )


class DecoderWrapper(torch.nn.Module):
    def __init__(self, module: torch.nn.Module) -> None:
        super().__init__()
        self.module = module

    def forward(
        self,
        targets: torch.Tensor,
        target_length: torch.Tensor,
        h_in: torch.Tensor,
        c_in: torch.Tensor,
    ):
        decoder, _, state = self.module(
            targets=targets.to(dtype=torch.long),
            target_length=target_length.to(dtype=torch.long),
            states=[h_in, c_in],
        )
        return decoder, state[0], state[1]


class JointDecisionV3(torch.nn.Module):
    """Single-step TDT joint and decision head used by FluidAudio v3."""

    def __init__(self, joint: torch.nn.Module, vocab_size: int, num_extra: int) -> None:
        super().__init__()
        self.joint = joint
        self.vocab_with_blank = int(vocab_size) + 1
        self.num_extra = int(num_extra)

    def forward(self, encoder_step: torch.Tensor, decoder_step: torch.Tensor):
        encoder = encoder_step.transpose(1, 2)
        decoder = decoder_step.transpose(1, 2)
        enc_proj = self.joint.enc(encoder)
        dec_proj = self.joint.pred(decoder)
        x = enc_proj.unsqueeze(2) + dec_proj.unsqueeze(1)
        x = self.joint.joint_net[0](x)
        x = self.joint.joint_net[1](x)
        logits = self.joint.joint_net[2](x)

        token_logits = logits[..., : self.vocab_with_blank]
        duration_logits = logits[..., -self.num_extra :]
        token_id = torch.argmax(token_logits, dim=-1).to(dtype=torch.int32)
        probabilities = torch.softmax(token_logits, dim=-1)
        token_prob = torch.gather(
            probabilities, -1, token_id.to(dtype=torch.long).unsqueeze(-1)
        ).squeeze(-1)
        duration = torch.argmax(duration_logits, dim=-1).to(dtype=torch.int32)
        top_k_logits, top_k_ids = torch.topk(
            token_logits, k=min(64, self.vocab_with_blank), dim=-1
        )
        return (
            token_id,
            token_prob,
            duration,
            top_k_ids.to(dtype=torch.int32),
            top_k_logits,
        )


def trace_inputs(window_s: int) -> Tuple[int, int, int]:
    if window_s not in (15, 30):
        raise ValueError("window_s must be 15 or 30")
    samples = window_s * 16_000
    mel_frames = 1501 if window_s == 15 else 3001
    encoder_frames = 188 if window_s == 15 else 376
    return samples, mel_frames, encoder_frames
