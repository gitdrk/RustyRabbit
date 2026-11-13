/* Negative control: uses CLZ, an ARMv5-only instruction. Must SIGILL on an ARMv4T model. */
static long sc(long n, long a){register long r0 __asm__("r0")=a; register long r7 __asm__("r7")=n;
  __asm__ volatile("swi 0":"+r"(r0):"r"(r7):"memory"); return r0;}
void _start(void){ unsigned x = 0x00100000, r;
  __asm__ volatile("clz %0, %1" : "=r"(r) : "r"(x));
  sc(1, (long)r); }   /* exit(clz result) = 11 */
