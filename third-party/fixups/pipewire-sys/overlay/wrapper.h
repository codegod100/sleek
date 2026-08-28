/*
 * The hermetic libclang invocation has Clang's resource headers but no host
 * libc sysroot. SPA uses these two C99 names without including stdint.h first,
 * so define them from Clang's target-width builtin types.
 */
typedef __UINT32_TYPE__ uint32_t;
typedef __UINTPTR_TYPE__ uintptr_t;

#include <pipewire/pipewire.h>
#include <pipewire/extensions/client-node.h>
#include <pipewire/extensions/metadata.h>
#include <pipewire/extensions/profiler.h>
#include <pipewire/extensions/protocol-native.h>
