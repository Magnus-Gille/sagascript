"""Read/change execution QoS only for an explicitly supplied lab child PID."""
import ctypes
from ctypes import wintypes


class PowerState(ctypes.Structure):
    _fields_ = [('Version', wintypes.DWORD), ('ControlMask', wintypes.DWORD),
                ('StateMask', wintypes.DWORD)]


def process_qos(pid, mode=None):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
    kernel.GetProcessInformation.restype = wintypes.BOOL
    kernel.SetProcessInformation.argtypes = kernel.GetProcessInformation.argtypes
    kernel.SetProcessInformation.restype = wintypes.BOOL
    handle = kernel.OpenProcess(0x1000 | (0x0200 if mode else 0), False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        state = PowerState(1, 0, 0)
        if not kernel.GetProcessInformation(handle, 4, ctypes.byref(state), ctypes.sizeof(state)):
            raise ctypes.WinError(ctypes.get_last_error())
        before = {'version': state.Version, 'control': state.ControlMask, 'state': state.StateMask}
        if mode:
            if mode not in ('high', 'eco', 'system'):
                raise ValueError(mode)
            state.ControlMask = (state.ControlMask | 1) if mode != 'system' else (state.ControlMask & ~1)
            state.StateMask = (state.StateMask | 1) if mode == 'eco' else (state.StateMask & ~1)
            if not kernel.SetProcessInformation(handle, 4, ctypes.byref(state), ctypes.sizeof(state)):
                raise ctypes.WinError(ctypes.get_last_error())
            if not kernel.GetProcessInformation(handle, 4, ctypes.byref(state), ctypes.sizeof(state)):
                raise ctypes.WinError(ctypes.get_last_error())
        after = {'version': state.Version, 'control': state.ControlMask, 'state': state.StateMask}
        return {'before': before, 'after': after, 'requested': mode}
    finally:
        kernel.CloseHandle(handle)
