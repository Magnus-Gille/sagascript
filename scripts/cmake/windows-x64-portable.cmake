# This hook is injected through CMAKE_PROJECT_INCLUDE by the Windows package
# build. Keep the policy scoped to whisper.cpp: other CMake projects may use
# different CPU targets and must not inherit this application's baseline.
if(NOT PROJECT_NAME STREQUAL "whisper.cpp")
  return()
endif()

string(TOLOWER "${CMAKE_SYSTEM_NAME}" _sagascript_system_name)
if(NOT _sagascript_system_name STREQUAL "windows")
  message(FATAL_ERROR
    "Sagascript whisper.cpp portability hook: CMAKE_SYSTEM_NAME must be Windows "
    "(got '${CMAKE_SYSTEM_NAME}')")
endif()

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" _sagascript_processor)
if(NOT _sagascript_processor STREQUAL "amd64"
   AND NOT _sagascript_processor STREQUAL "x86_64")
  message(FATAL_ERROR
    "Sagascript whisper.cpp portability hook requires an AMD64 or x86_64 target "
    "(got '${CMAKE_SYSTEM_PROCESSOR}')")
endif()

if(DEFINED CMAKE_SIZEOF_VOID_P AND NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
  message(FATAL_ERROR
    "Sagascript whisper.cpp portability hook requires 8-byte pointers "
    "for the x64 target (got '${CMAKE_SIZEOF_VOID_P}')")
endif()

# This is the shipped x64 inference baseline, not a universal compatibility
# switch for every old x64 CPU. Do not enable build-host-native instructions or
# AVX-512/AMX: the resulting package must run on the supported baseline.
set(GGML_NATIVE OFF CACHE BOOL "Disable host-native GGML instructions" FORCE)
set(GGML_AVX ON CACHE BOOL "Enable GGML AVX" FORCE)
set(GGML_SSE42 ON CACHE BOOL "Enable GGML SSE4.2" FORCE)
set(GGML_AVX2 ON CACHE BOOL "Enable GGML AVX2" FORCE)
set(GGML_BMI2 ON CACHE BOOL "Enable GGML BMI2" FORCE)
set(GGML_FMA ON CACHE BOOL "Enable GGML FMA" FORCE)
set(GGML_F16C ON CACHE BOOL "Enable GGML F16C" FORCE)

set(GGML_AVX_VNNI OFF CACHE BOOL "Disable GGML AVX-VNNI" FORCE)
set(GGML_AVX512 OFF CACHE BOOL "Disable GGML AVX-512" FORCE)
set(GGML_AVX512_VBMI OFF CACHE BOOL "Disable GGML AVX-512 VBMI" FORCE)
set(GGML_AVX512_VNNI OFF CACHE BOOL "Disable GGML AVX-512 VNNI" FORCE)
set(GGML_AVX512_BF16 OFF CACHE BOOL "Disable GGML AVX-512 BF16" FORCE)
set(GGML_AMX_TILE OFF CACHE BOOL "Disable GGML AMX tile" FORCE)
set(GGML_AMX_INT8 OFF CACHE BOOL "Disable GGML AMX INT8" FORCE)
set(GGML_AMX_BF16 OFF CACHE BOOL "Disable GGML AMX BF16" FORCE)
set(GGML_CPU_ALL_VARIANTS OFF CACHE BOOL "Disable all GGML CPU variants" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "Disable dynamically loaded GGML backends" FORCE)
set(GGML_LLAMAFILE OFF CACHE BOOL "Disable the GGML llamafile backend" FORCE)
