/*===- InstrProfilingPort.h- Support library for PGO instrumentation ------===*\
|*
|* Part of the LLVM Project, under the Apache License v2.0 with LLVM Exceptions.
|* See https://llvm.org/LICENSE.txt for license information.
|* SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
|*
\*===----------------------------------------------------------------------===*/

/* This header must be included after all others so it can provide fallback
   definitions for stuff missing in system headers. */

#ifndef PROFILE_INSTRPROFILING_PORT_H_
#define PROFILE_INSTRPROFILING_PORT_H_

#include <stdalign.h>
#include <stddef.h>
#include <stdint.h>

#ifdef _MSC_VER
#define COMPILER_RT_ALIGNAS(x) __declspec(align(x))
#define COMPILER_RT_VISIBILITY
/* FIXME: selectany does not have the same semantics as weak. */
#define COMPILER_RT_WEAK __declspec(selectany)
#define COMPILER_RT_ALLOCA __builtin_alloca
/* Need to include <stdio.h> and <io.h> */
#define COMPILER_RT_FTRUNCATE(f,l) _chsize(_fileno(f),l)
#define COMPILER_RT_ALWAYS_INLINE __forceinline
#define COMPILER_RT_CLEANUP(x)
#define COMPILER_RT_USED
#elif __GNUC__
#ifdef _WIN32
#define COMPILER_RT_FTRUNCATE(f, l) _chsize(fileno(f), l)
#define COMPILER_RT_VISIBILITY
#define COMPILER_RT_WEAK __attribute__((selectany))
#else
#define COMPILER_RT_FTRUNCATE(f, l) ftruncate(fileno(f), l)
#define COMPILER_RT_VISIBILITY __attribute__((visibility("hidden")))
#define COMPILER_RT_WEAK __attribute__((weak))
#endif
#define COMPILER_RT_ALIGNAS(x) __attribute__((aligned(x)))
#define COMPILER_RT_ALLOCA __builtin_alloca
#define COMPILER_RT_ALWAYS_INLINE inline __attribute((always_inline))
#define COMPILER_RT_CLEANUP(x) __attribute__((cleanup(x)))
#define COMPILER_RT_USED __attribute__((used))
#endif

#if defined(__APPLE__)
#define COMPILER_RT_SEG "__DATA,"
#else
#define COMPILER_RT_SEG ""
#endif

#ifdef _MSC_VER
#define COMPILER_RT_SECTION(Sect) __declspec(allocate(Sect))
#else
#define COMPILER_RT_SECTION(Sect) __attribute__((section(Sect)))
#endif

#define COMPILER_RT_MAX_HOSTLEN 128
#if defined(__ORBIS__) || defined(__wasi__)
#define COMPILER_RT_GETHOSTNAME(Name, Len) ((void)(Name), (void)(Len), (-1))
#else
#define COMPILER_RT_GETHOSTNAME(Name, Len) lprofGetHostName(Name, Len)
#endif

#if COMPILER_RT_HAS_ATOMICS == 1
#define COMPILER_RT_BOOL_CMPXCHG(Ptr, OldV, NewV)                              \
  __sync_bool_compare_and_swap(Ptr, OldV, NewV)
#define COMPILER_RT_PTR_FETCH_ADD(DomType, PtrVar, PtrIncr)                    \
  (DomType *)__sync_fetch_and_add((intptr_t *)&PtrVar,                         \
                                  sizeof(DomType) * PtrIncr)
#else /* COMPILER_RT_HAS_ATOMICS != 1 */
#include "InstrProfilingUtil.h"
#define COMPILER_RT_BOOL_CMPXCHG(Ptr, OldV, NewV)                              \
  lprofBoolCmpXchg((void **)Ptr, OldV, NewV)
#define COMPILER_RT_PTR_FETCH_ADD(DomType, PtrVar, PtrIncr)                    \
  (DomType *)lprofPtrFetchAdd((void **)&PtrVar, sizeof(DomType) * PtrIncr)
#endif

#if defined(_WIN32)
#define DIR_SEPARATOR '\\'
#define DIR_SEPARATOR_2 '/'
#else
#define DIR_SEPARATOR '/'
#endif

#ifndef DIR_SEPARATOR_2
#define IS_DIR_SEPARATOR(ch) ((ch) == DIR_SEPARATOR)
#else /* DIR_SEPARATOR_2 */
#define IS_DIR_SEPARATOR(ch)                                                   \
  (((ch) == DIR_SEPARATOR) || ((ch) == DIR_SEPARATOR_2))
#endif /* DIR_SEPARATOR_2 */

static inline size_t getpagesize(void) {
  /* Minicov does not support continuous mode. */
  return 1;
}

#ifdef COMPILER_RT_PROFILE_BAREMETAL
// Baremetal doesn't support logging
#define PROF_ERR(Format, ...)
#define PROF_WARN(Format, ...)
#define PROF_NOTE(Format, ...)
#else
#define PROF_ERR(Format, ...)                                                  \
  fprintf(stderr, "LLVM Profile Error: " Format, __VA_ARGS__);

#define PROF_WARN(Format, ...)                                                 \
  fprintf(stderr, "LLVM Profile Warning: " Format, __VA_ARGS__);

#define PROF_NOTE(Format, ...)                                                 \
  fprintf(stderr, "LLVM Profile Note: " Format, __VA_ARGS__);
#endif /* COMPILER_RT_PROFILE_BAREMETAL */

#ifndef MAP_FILE
#define MAP_FILE 0
#endif

#ifndef O_BINARY
#define O_BINARY 0
#endif

#define memcmp __builtin_memcmp
#define memset __builtin_memset
#define memcpy __builtin_memcpy
#define memmove __builtin_memmove
#define assert(...) ((void)0)

void *minicov_alloc_zeroed(size_t Size, size_t Alignment);
void minicov_dealloc(void *Ptr, size_t Size, size_t Alignment);

#endif /* PROFILE_INSTRPROFILING_PORT_H_ */
