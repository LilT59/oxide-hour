#!/usr/bin/env python3
"""Regenerate the sin/cos table of `crates/zr-sim/src/crtmath.rs` from mathematics.

Usage: python -I tools/gen_crtmath_table.py            # print the Rust table
       python -I tools/gen_crtmath_table.py --check FILE   # compare with the file's table

The C++ build's `Sin`/`Cos` are `(float)sin((double)x)` / `(float)cos((double)x)` on 32-bit x86
(`corecrt_math.h`), and the double routines the build imports are `_libm_sse2_sin_precise` /
`_libm_sse2_cos_precise`, which use a Cody-Waite reduction by `pi/32`, a table of `sin`/`cos` at
multiples of `pi/32`, and a degree-9/8 polynomial for the remainder. `crtmath.rs` reproduces that
algorithm; this script derives its table from mathematics only - no value is read out of any binary.

`TABLE[k]` is `{cos_mid, sin_hi, sin_low, cos_hi}` for `theta = k*pi/32`, k = 0..63:

  sin_hi  = the correctly rounded double of sin(theta)
  cos_hi  = the power of two 2^n with n the smallest integer >= log2|cos(theta)| - 0.5 (the nearest
            power of two in ratio, ties to the smaller one), or 0 where cos(theta) == 0
  cos_mid = the correctly rounded double of cos(theta) - cos_hi
  sin_low = the correctly rounded double of sin(theta) - sin_hi

theta is evaluated with the Taylor series at 90-digit precision (pi given to 90 digits); at the
multiples of pi/4 (k = 0, 8, ..., 56) the exact algebraic values 0, +/-1, +/-sqrt(2)/2 are used, so the
sqrt(2)/2 ties in the `cos_hi` rule are exact. Python's `float(Decimal)` rounds correctly, which is
what "correctly rounded double" means here.

No game data, no external packages. Prints to stdout; `--check` exits 1 when the file's table differs
(a few seconds at 90 digits, run by hand when the table is touched).
"""

import argparse
import re
import sys
from decimal import Decimal, getcontext

# 90 significant digits of pi (more than the 90-digit working precision keeps the series exact enough).
PI = Decimal(
    "3.14159265358979323846264338327950288419716939937510582097494459230781640628620899862803482534"
)


def sin_series(theta):
    """sin(theta) by its Taylor series at the current Decimal precision."""
    total, term, k = Decimal(0), theta, 1
    while True:
        total += term
        term = -term * theta * theta / ((k + 1) * (k + 2))
        k += 2
        if abs(term) < Decimal(10) ** -86:
            return total


def cos_series(theta):
    """cos(theta) by its Taylor series at the current Decimal precision."""
    total, term, k = Decimal(0), Decimal(1), 0
    while True:
        total += term
        term = -term * theta * theta / ((k + 1) * (k + 2))
        k += 2
        if abs(term) < Decimal(10) ** -86:
            return total


def power_of_two_near(cos):
    """2^n with n the smallest integer >= log2|cos| - 0.5 (ties to the smaller power), 0 for cos 0."""
    if cos == 0:
        return 0.0
    square = cos.copy_abs() ** 2
    n = -20
    while square > Decimal(2) ** (2 * n + 1):
        n += 1
    value = float(Decimal(2) ** n)
    return -value if cos < 0 else value


def table():
    """The 64 table rows, each `[cos_mid, sin_hi, sin_low, cos_hi]`."""
    getcontext().prec = 90
    sqrt_half = Decimal(2).sqrt() / 2
    exact = {
        0: (Decimal(0), Decimal(1)),
        8: (sqrt_half, sqrt_half),
        16: (Decimal(1), Decimal(0)),
        24: (sqrt_half, -sqrt_half),
        32: (Decimal(0), Decimal(-1)),
        40: (-sqrt_half, -sqrt_half),
        48: (Decimal(-1), Decimal(0)),
        56: (-sqrt_half, sqrt_half),
    }
    rows = []
    for k in range(64):
        if k in exact:
            sin, cos = exact[k]
        else:
            theta = Decimal(k) * PI / 32
            sin, cos = sin_series(theta), cos_series(theta)
        sin_hi = float(sin)
        cos_hi = power_of_two_near(cos)
        cos_mid = float(cos - Decimal(cos_hi))
        sin_low = float(sin - Decimal(sin_hi))
        rows.append([
            0.0 if cos_mid == 0.0 else cos_mid,
            0.0 if sin_hi == 0.0 else sin_hi,
            0.0 if sin_low == 0.0 else sin_low,
            0.0 if cos_hi == 0.0 else cos_hi,
        ])
    return rows


def rust_rows(rows):
    return ["    [%r, %r, %r, %r]," % tuple(row) for row in rows]


def main(argv=None):
    parser = argparse.ArgumentParser(description="Derive the crtmath sin/cos table from mathematics.")
    parser.add_argument("--check", metavar="FILE", help="compare with the table in this source file")
    args = parser.parse_args(argv)

    wanted = rust_rows(table())
    if not args.check:
        print("\n".join(wanted))
        return 0

    try:
        text = open(args.check, encoding="utf-8").read()
    except OSError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    match = re.search(r"static TABLE: \[\[f64; 4\]; 64\] = \[\n(.*?)\n\];", text, re.S)
    if not match:
        print(f"error: no TABLE in {args.check}", file=sys.stderr)
        return 2
    have = [line for line in match.group(1).splitlines() if line.strip()]
    if [line.strip() for line in have] != [line.strip() for line in wanted]:
        print(f"error: {args.check}'s table differs from the derived one", file=sys.stderr)
        for i, (a, b) in enumerate(zip(have, wanted)):
            if a.strip() != b.strip():
                print(f"  row {i}: file {a.strip()} != derived {b.strip()}", file=sys.stderr)
        return 1
    print(f"{args.check}: table matches the derived one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
