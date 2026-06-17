`bitrs` is a no-std crate for ergonomically specifying layouts of bitfields
over integral types. While the aim is to be general-purpose, the imagined user
is a systems programmer uncomfortably hunched over an architectural manual or
hardware spec, looking to transcribe register layouts into Rust with minimal
fuss.

The heavy lifting is done by the `layout!` procedural macro. Care
is taken to generate readable and efficient code. The [`zerocopy`][zerocopy]
crate is further leveraged for safe and efficient transmutation between integral
values and custom bitfield representations.

## Features

* Generation of a simple, extensible wrapper type around the given integral base
  type;
* Simple, const-friendly builder pattern for constructing layout instances;
* Automatic implementations of the basic, convenient traits one would expect out
  of a thin integral wrapper type:
    * `Copy`, `Clone`
    * `Eq`, `PartialEq`
    * `Default`
    * `From` over the base type
    * `Deref` and `DerefMut` with a target of the underlying base type
    * `Debug`, `Binary`, `LowerHex`, `UpperHex`, `Octal`
* Specification of default and "reserved-as" values, with `new()` respecting
  reserved-as values and `default()` respecting both;
* Custom bitfield representation types without any boilerplate;
* Iteration over individual bitfield metadata and values;
* Associated constants around masks and shifts for use in inline assembly.

For more detail, see `layout!`.

## Example

```rust
use bitrs::{bitfield_repr, layout};

#[bitfield_repr(u8)]
pub enum CustomFieldRepr {
    Option1 = 0xa,
    Option2 = 0xf,
}

layout!({
    pub struct Example(u32);
    {
        let foo @ 21..14;
        let custom @ 13..10: CustomFieldRepr;
        let bar @ 9..8 = 0b11;
        let baz @ 7;
        let frob @ 6..4;
        let __ @ 3..2 = 1;
        let __ @ 1..0;
    }
});

fn main() {
    let example = *Example::default()
        .set_custom(CustomFieldRepr::Option2)
        .set_frob(0x7);
    assert_eq!(example.foo(), 0);
    assert_eq!(example.custom(), CustomFieldRepr::Option2);
    assert_eq!(example.bar(), 0b11);
    assert_eq!(example.baz(), false);
    assert_eq!(example.frob(), 0x7);
    assert_eq!(*example & 0b1100, 0b0100);

    // Will print: `Example { custom: Option2, foo: 0x0, bar: 0x3, baz: false, frob: 0xa }`
    println!("{example}");

    // Or iterate over all fields and and print them individually.
    for (metadata, value) in example {
      println!("{}: {:x}", metadata.name, value);
    }
}
```

`bitfield_repr` is an attribute that is syntactic sugar for deriving
`repr(X)` and the handful of traits expected of a custom field representation.

## Why another crate for bitfields?
There are already a handful out there, so why this one too? It is the author's
opinion that none of those at the time of writing this offer _all_ of the above
features (e.g., around reserved semantics or boilerplate-free, custom field
representations) or the author's desired ergonomics around register modeling.
For example, some constrain field specification by bit width instead of by an
explicit bit range, which is not how registers are commonly described in
official references (plus, the author surely can't trust himself to do mental
math like that).

[zerocopy]: https://docs.rs/zerocopy/latest/zerocopy/
