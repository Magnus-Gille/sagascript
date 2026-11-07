# Injected through CMAKE_PROJECT_INCLUDE by the macOS CI and release builds
# (scripts/ci/use-portable-whisper-macos.sh). whisper-rs-sys forwards CMAKE_*
# environment variables to whisper.cpp's CMake, but not GGML_* ones. Keep the
# policy scoped to whisper.cpp: other CMake projects must not inherit it.
if(NOT PROJECT_NAME STREQUAL "whisper.cpp")
  return()
endif()

if(NOT CMAKE_SYSTEM_NAME STREQUAL "Darwin")
  message(FATAL_ERROR
    "Sagascript whisper.cpp macOS hook: CMAKE_SYSTEM_NAME must be Darwin "
    "(got '${CMAKE_SYSTEM_NAME}')")
endif()

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" _sagascript_processor)
if(NOT _sagascript_processor STREQUAL "arm64"
   AND NOT _sagascript_processor STREQUAL "aarch64")
  message(FATAL_ERROR
    "Sagascript whisper.cpp macOS hook requires an Apple silicon target "
    "(got '${CMAKE_SYSTEM_PROCESSOR}')")
endif()

# GGML_NATIVE tunes ggml to the build machine (-mcpu=native plus run-time
# probes). A shipped binary must not depend on which CPU the CI runner has,
# and on the macos-15 runners (AppleClang 17) the probe is inconsistent: it
# found that the runner cannot run i8mm and appended +noi8mm, yet ggml still
# reported MATMUL_INT8 and compiled its i8mm kernels, so whisper-rs-sys
# 0.14.1 failed to build ("vmmlaq_s32 requires target feature 'i8mm'").
#
# With GGML_NATIVE off and no GGML_CPU_ARM_ARCH, ggml passes no -mcpu or
# -march, and Apple clang targets its arm64 macOS default, apple-m1 (dotprod,
# fp16, no i8mm): the oldest Apple silicon Sagascript supports, and the same
# target the M1 macos-14 runners produced natively. Re-audit on whisper-rs-sys
# updates.
set(GGML_NATIVE OFF CACHE BOOL "Disable host-native GGML instructions" FORCE)
set(GGML_CPU_ALL_VARIANTS OFF CACHE BOOL "Disable all GGML CPU variants" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "Disable dynamically loaded GGML backends" FORCE)
