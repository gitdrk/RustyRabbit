/* Sanity check for librt_armv4t.a (run by scripts/setup-cc.sh under qemu-arm -cpu ti925t):
 * integer division, 64-bit division, soft-float and clz must all work on an ARMv4T core.
 * Usage: rtlib-check 100 12345678901
 * Expect: 14 2 123456789 1 50.000000 123456789.010000 25 */
#include <stdio.h>
#include <stdlib.h>
int main(int c, char **v) {
    unsigned a = atoi(v[1]); long long b = atoll(v[2]); float f = atof(v[1]); double d = atof(v[2]);
    printf("%u %u %lld %lld %f %f %d\n", a / 7, a % 7, b / a, b % a, f * 1.5f / 3.0f, d / f, __builtin_clz(a));
    return 0;
}
