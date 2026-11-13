#!/bin/bash
# Static check: disassemble an UNSTRIPPED binary (so $a/$d mapping symbols keep
# literal pools from being decoded as code) and list every instruction newer
# than ARMv4T, grouped by the function it lives in.
# Complements the qemu-arm ti925t run, which only catches code paths that execute.
set -uo pipefail
BIN=${1:?unstripped binary}
# ARMv5+: blx clz ldrd strd pld qadd/qsub smlaXY/smulXY/smulw; ARMv6+: ldrex strex rev uxt sxt;
# v6T2/v7: movw movt ubfx sbfx bfi bfc dmb dsb isb cbz it; VFP/NEON: v*; coproc v5: *2.
# Not flagged: smull/umull/smlal/umlal (ARMv4 long multiply), udf #65006 (LLVM trap).
PAT='^(blx|clz|ldrd|strd|pld|q(d?add|d?sub)|smla[bt][bt]|smlaw[bt]|smlal[bt][bt]|smlsd|smlad|smul[bt][bt]|smulw[bt]|smmul|ldrex|strex|clrex|rev|[us]xt|movw|movt|[us]bfx|bfi|bfc|dmb|dsb|isb|cbn?z|it[te]*$|v[a-z]+|ldc2|stc2|mcr2|mrc2|cdp2|mcrr|mrrc|bkpt)'
arm-linux-gnueabi-objdump -d --no-show-raw-insn "$BIN" | awk -v pat="$PAT" '
  /^[0-9a-f]+ <.*>:$/ { fn=$2; next }
  # musl picks these at startup only if the kernel reports HWCAP_TLS (ARMv6+).
  # On the ARM920T it uses the kernel kuser helpers instead, so they never run.
  fn ~ /^<__a_(cas|barrier)_v[67]>:$/ { allowed++; next }
  /^ +[0-9a-f]+:/ {
    m=$2
    if (m ~ /^[.]/) next                    # .word etc = data
    if (m=="udf" && $3=="#65006") { trap++; next }
    n++
    if (m ~ pat) { bad[fn" "m]++; total++ }
  }
  END {
    printf "code instructions: %d (LLVM trap udf: %d, musl v6/v7-only atomics skipped: %d lines)\n", n, trap, allowed
    if (total==0) { print "OK: nothing newer than ARMv4T"; exit 0 }
    printf "NEWER-THAN-ARMv4T: %d\n", total
    for (k in bad) printf "  %4d  %s\n", bad[k], k
    exit 1
  }'
