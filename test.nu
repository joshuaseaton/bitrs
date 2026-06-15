#!/usr/bin/env nu

# Copyright (c) 2026 Joshua Seaton
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT#

cargo test

# Make sure examples compile too.
cargo run --example basic
cargo run --example bitfield-repr
cargo run --example multilayout
