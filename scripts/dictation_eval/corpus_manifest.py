"""Pure schema and coverage validation for the offline evaluation manifest."""

import re
from collections.abc import Mapping


_TOP_LEVEL_KEYS = {"schema_version", "utterances"}
_ROW_KEYS = {
    "id",
    "language",
    "split",
    "speaker_id",
    "audio_sha256",
    "reference_sha256",
    "origin",
    "duration_bucket",
    "environment",
    "tags",
}
_ID_PATTERN = re.compile(r"[A-Za-z0-9_-]{1,80}")
_SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
_LANGUAGES = ("en", "sv", "no")
_SPLITS = {"dev", "heldout"}
_ORIGINS = {"human", "synthetic", "silence"}
_DURATION_BUCKETS = ("short", "medium", "long")
_ENVIRONMENTS = ("quiet", "noisy")
_COVERAGE_TAGS = ("specialist", "numbers", "negation", "ordinary")
_ALLOWED_TAGS = set(_COVERAGE_TAGS) | {"silence"}


def _require_string(value: object, name: str) -> str:
    if type(value) is not str:
        raise ValueError(f"{name} must be a string")
    return value


def _require_enum(value: object, name: str, allowed: set[str] | tuple[str, ...]) -> str:
    value = _require_string(value, name)
    if value not in allowed:
        raise ValueError(f"{name} is invalid")
    return value


def _require_opaque_id(value: object, name: str) -> str:
    value = _require_string(value, name)
    if _ID_PATTERN.fullmatch(value) is None:
        raise ValueError(f"{name} is invalid")
    return value


def _require_sha256(value: object, name: str) -> str:
    value = _require_string(value, name)
    if _SHA256_PATTERN.fullmatch(value) is None:
        raise ValueError(f"{name} is invalid")
    return value


def validate_manifest(value: object) -> dict[str, object]:
    """Validate and copy a version-one evaluation manifest."""

    if not isinstance(value, Mapping):
        raise ValueError("manifest must be an object")
    if set(value) != _TOP_LEVEL_KEYS:
        raise ValueError("manifest has unexpected or missing keys")
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ValueError("schema_version must be one")
    utterances = value["utterances"]
    if type(utterances) is not list or not 1 <= len(utterances) <= 500:
        raise ValueError("utterances must contain between one and 500 rows")

    copied_utterances: list[dict[str, object]] = []
    seen_ids: set[str] = set()
    seen_audio_hashes: set[str] = set()
    for row in utterances:
        if not isinstance(row, Mapping) or set(row) != _ROW_KEYS:
            raise ValueError("utterance has unexpected or missing keys")
        utterance_id = _require_opaque_id(row["id"], "id")
        if utterance_id in seen_ids:
            raise ValueError("utterance IDs must be unique")
        seen_ids.add(utterance_id)
        language = _require_enum(row["language"], "language", _LANGUAGES)
        split = _require_enum(row["split"], "split", _SPLITS)
        speaker_id = _require_opaque_id(row["speaker_id"], "speaker_id")
        audio_sha256 = _require_sha256(row["audio_sha256"], "audio_sha256")
        if audio_sha256 in seen_audio_hashes:
            raise ValueError("audio hashes must be unique")
        seen_audio_hashes.add(audio_sha256)
        reference_sha256 = _require_sha256(row["reference_sha256"], "reference_sha256")
        origin = _require_enum(row["origin"], "origin", _ORIGINS)
        duration_bucket = _require_enum(
            row["duration_bucket"], "duration_bucket", _DURATION_BUCKETS
        )
        environment = _require_enum(row["environment"], "environment", _ENVIRONMENTS)
        tags = row["tags"]
        if type(tags) is not list:
            raise ValueError("tags must be a list")
        copied_tags: list[str] = []
        seen_tags: set[str] = set()
        for tag in tags:
            tag = _require_enum(tag, "tag", _ALLOWED_TAGS)
            if tag in seen_tags:
                raise ValueError("tags must be unique")
            seen_tags.add(tag)
            copied_tags.append(tag)
        if (origin == "silence") != ("silence" in seen_tags):
            raise ValueError("silence origin and tag must agree")
        copied_utterances.append(
            {
                "id": utterance_id,
                "language": language,
                "split": split,
                "speaker_id": speaker_id,
                "audio_sha256": audio_sha256,
                "reference_sha256": reference_sha256,
                "origin": origin,
                "duration_bucket": duration_bucket,
                "environment": environment,
                "tags": copied_tags,
            }
        )

    return {"schema_version": 1, "utterances": copied_utterances}


def coverage_report(validated: Mapping[str, object]) -> dict[str, object]:
    """Return content-free prerequisite coverage for a validated manifest."""

    manifest = validate_manifest(validated)
    rows = manifest["utterances"]
    assert isinstance(rows, list)
    languages: dict[str, dict[str, object]] = {}
    eligible = True
    for language in _LANGUAGES:
        human_dev = [
            row
            for row in rows
            if row["language"] == language
            and row["split"] == "dev"
            and row["origin"] == "human"
        ]
        human_heldout = [
            row
            for row in rows
            if row["language"] == language
            and row["split"] == "heldout"
            and row["origin"] == "human"
        ]
        human_rows = human_dev + human_heldout
        heldout_rows = [
            row for row in rows if row["language"] == language and row["split"] == "heldout"
        ]
        heldout_human_tags = {
            tag for row in human_heldout for tag in row["tags"] if tag in _COVERAGE_TAGS
        }
        heldout_human_durations = {row["duration_bucket"] for row in human_heldout}
        heldout_human_environments = {row["environment"] for row in human_heldout}
        missing_tags = [tag for tag in _COVERAGE_TAGS if tag not in heldout_human_tags]
        missing_durations = [
            bucket for bucket in _DURATION_BUCKETS if bucket not in heldout_human_durations
        ]
        missing_environments = [
            environment
            for environment in _ENVIRONMENTS
            if environment not in heldout_human_environments
        ]
        heldout_silence = sum(1 for row in heldout_rows if row["origin"] == "silence")
        human_speakers = len({row["speaker_id"] for row in human_rows})
        heldout_human_speakers = len({row["speaker_id"] for row in human_heldout})
        language_eligible = (
            len(human_dev) >= 10
            and len(human_heldout) >= 40
            and human_speakers >= 2
            and heldout_human_speakers >= 2
            and not missing_tags
            and not missing_durations
            and not missing_environments
            and heldout_silence >= 1
        )
        eligible = eligible and language_eligible
        languages[language] = {
            "dev_human": len(human_dev),
            "heldout_human": len(human_heldout),
            "human_speakers": human_speakers,
            "heldout_human_speakers": heldout_human_speakers,
            "missing_coverage_tags": missing_tags,
            "missing_duration_buckets": missing_durations,
            "missing_environments": missing_environments,
            "heldout_silence": heldout_silence,
            "eligible": language_eligible,
        }

    return {
        "schema_version": 1,
        "eligible": eligible,
        "languages": languages,
    }
