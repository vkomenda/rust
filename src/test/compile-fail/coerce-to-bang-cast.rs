// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![feature(never_type)]

fn foo(x: usize, y: !, z: usize) { }

#[deny(coerce_never)]
fn cast_a() {
    let y = {return; 22} as !;
    //~^ ERROR cannot coerce `i32` to !
    //~| hard error
}

fn cast_b() {
    let y = 22 as !; //~ ERROR non-primitive cast
}

fn main() { }
