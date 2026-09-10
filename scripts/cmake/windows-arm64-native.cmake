# Injected through CMAKE_PROJECT_INCLUDE by the unsigned Windows ARM64
# candidate. Other CMake projects and all other workflow targets are unaffected.
if(NOT PROJECT_NAME STREQUAL "whisper.cpp")
  return()
endif()

string(TOLOWER "${CMAKE_SYSTEM_NAME}" _sagascript_system_name)
if(NOT _sagascript_system_name STREQUAL "windows")
  message(FATAL_ERROR
    "Sagascript whisper.cpp ARM hook: CMAKE_SYSTEM_NAME must be Windows "
    "(got '${CMAKE_SYSTEM_NAME}')")
endif()

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" _sagascript_processor)
if(NOT _sagascript_processor STREQUAL "arm64"
   AND NOT _sagascript_processor STREQUAL "aarch64")
  message(FATAL_ERROR
    "Sagascript whisper.cpp ARM hook requires an ARM64 or aarch64 target "
    "(got '${CMAKE_SYSTEM_PROCESSOR}')")
endif()

if(DEFINED CMAKE_SIZEOF_VOID_P AND NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
  message(FATAL_ERROR
    "Sagascript whisper.cpp ARM hook requires 8-byte pointers "
    "(got '${CMAKE_SIZEOF_VOID_P}')")
endif()

# whisper-rs-sys 0.14.1 / Whisper 1.7.6 uses these cached source-run checks
# in ggml/src/ggml-cpu/CMakeLists.txt. Its SVE implementation includes Linux's
# sys/prctl.h, even when a Windows Clang probe reports success. Skip only the
# positive SVE/SME probes: GGML still verifies +nosve/+nosme compiler support
# and probes dotprod/i8mm normally. Keep GGML_NATIVE and other tuning unchanged.
# These are pinned implementation details; re-audit them on dependency updates.
set(GGML_MACHINE_SUPPORTS_sve OFF CACHE INTERNAL "SVE unavailable in Windows Whisper" FORCE)
set(GGML_MACHINE_SUPPORTS_sme OFF CACHE INTERNAL "SME unavailable in Windows Whisper" FORCE)
