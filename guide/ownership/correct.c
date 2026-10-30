#include <stdlib.h>

void consume(int *p)
/*@ requires take original = RW<int>(p);
    ensures true;
@*/
{
    volatile int *temporary = malloc(sizeof(int));
    if (temporary != NULL) {
        *temporary = *p;
        free((void *)temporary);
    }
    free(p);
}
