/* Minimal libunwind replacement for karotzd (built with panic=abort).
 * Nothing ever unwinds; std only references these for printing backtraces
 * and through .ARM.exidx personality entries. Each reports failure, so a
 * panic prints its message without a backtrace and then aborts. */
int _Unwind_Backtrace(void *fn, void *arg) { (void)fn; (void)arg; return 5; /* _URC_END_OF_STACK */ }
int _Unwind_VRS_Get(void *ctx, int cls, unsigned reg, int rep, void *val)
{ (void)ctx; (void)cls; (void)reg; (void)rep; (void)val; return 2; /* _UVRSR_FAILED */ }
int __aeabi_unwind_cpp_pr0(int st, void *ucb, void *ctx) { (void)st; (void)ucb; (void)ctx; return 9; /* _URC_FAILURE */ }
int __aeabi_unwind_cpp_pr1(int st, void *ucb, void *ctx) { (void)st; (void)ucb; (void)ctx; return 9; }
int __aeabi_unwind_cpp_pr2(int st, void *ucb, void *ctx) { (void)st; (void)ucb; (void)ctx; return 9; }
