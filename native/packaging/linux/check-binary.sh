#!/usr/bin/env bash
# Checks what a Linux binary asks of the system it will run on, so that a
# change of build machine or of a dependency cannot quietly raise it.
#
#   check-binary.sh BINARY
#
# The newest C library symbol it needs must not be newer than GLIBC_2.35 (the
# baseline named in the package and the release notes), and it may link only
# the C runtime: window system and graphics libraries are loaded while it
# runs and are listed by hand as package dependencies. Open CAD Studio also
# links the C++ runtime, for a mesh library written in C++; its newest symbol
# must not be newer than GLIBCXX_3.4.30, which the systems of that baseline
# have, and libstdc++6 is a dependency of the package.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

binary=${1:?usage: check-binary.sh BINARY}
baseline=2.35
cxx_baseline=3.4.30

newest=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
[[ -n "$newest" ]] || fail "no C library symbol versions found in $binary"
echo "newest C library symbol version: GLIBC_$newest (baseline GLIBC_$baseline)"
if [[ "$(printf '%s\n' "$newest" "$baseline" | sort -V | tail -1)" != "$baseline" ]]; then
    fail "$binary needs GLIBC_$newest, newer than the baseline GLIBC_$baseline"
fi

newest=$(objdump -T "$binary" | grep -o 'GLIBCXX_[0-9.]*' | sed 's/GLIBCXX_//' | sort -uV | tail -1 || true)
if [[ -n "$newest" ]]; then
    echo "newest C++ library symbol version: GLIBCXX_$newest (baseline GLIBCXX_$cxx_baseline)"
    if [[ "$(printf '%s\n' "$newest" "$cxx_baseline" | sort -V | tail -1)" != "$cxx_baseline" ]]; then
        fail "$binary needs GLIBCXX_$newest, newer than the baseline GLIBCXX_$cxx_baseline"
    fi
fi

needed=$(readelf -d "$binary" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
echo "linked libraries:"
echo "$needed" | sed 's/^/  /'
unexpected=$(echo "$needed" | grep -v -E '^(libc\.so\.6|libm\.so\.6|libgcc_s\.so\.1|libdl\.so\.2|libpthread\.so\.0|librt\.so\.1|libstdc\+\+\.so\.6|ld-linux-x86-64\.so\.2|ld-linux-aarch64\.so\.1)$' || true)
if [[ -n "$unexpected" ]]; then
    fail "$binary links libraries outside the C and C++ runtimes; add them to the Depends of linux/control.in and to this list: $unexpected"
fi
