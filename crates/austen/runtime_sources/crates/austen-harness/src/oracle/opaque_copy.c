#include <stddef.h>
#include <string.h>

#if defined(_MSC_VER)
#define AUSTEN_NOINLINE __declspec(noinline)
#elif defined(__GNUC__) || defined(__clang__)
#define AUSTEN_NOINLINE __attribute__((noinline))
#else
#define AUSTEN_NOINLINE
#endif

AUSTEN_NOINLINE void austen_oracle_copy_object_representation_v1(
    unsigned char *destination, const void *source, size_t length) {
  memcpy(destination, source, length);
}
