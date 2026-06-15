// Copyright (c) 2025 Joshua Seaton
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[cfg(test)]
mod tests {
    use bitfld::{FieldMetadata, bitfield_repr, layout};

    layout!({
        struct EmptyU8(u8);
        {}
    });

    layout!({
        struct OneFieldU16(u16);
        {
            let a @ 15..0;
        }
    });

    layout!({
        struct TwoFieldsU32(u32);
        {
            let a @ 31..16;
            let b @ 15..0;
        }
    });

    layout!({
        struct ThreeFieldsU64(u64);
        {
            let a @ 63..32;
            let b @ 31..16;
            let c @ 15..0;
        }
    });

    layout!({
        struct FourFieldsU128(u128);
        {
            let a @ 127..96;
            let b @ 95..64;
            let c @ 63..32;
            let d @ 31..0;
        }
    });

    #[test]
    fn size_and_alignment() {
        assert_eq!(size_of::<EmptyU8>(), size_of::<u8>());
        assert_eq!(align_of::<EmptyU8>(), align_of::<u8>());

        assert_eq!(size_of::<OneFieldU16>(), size_of::<u16>());
        assert_eq!(align_of::<OneFieldU16>(), align_of::<u16>());

        assert_eq!(size_of::<TwoFieldsU32>(), size_of::<u32>());
        assert_eq!(align_of::<TwoFieldsU32>(), align_of::<u32>());

        assert_eq!(size_of::<ThreeFieldsU64>(), size_of::<u64>());
        assert_eq!(align_of::<ThreeFieldsU64>(), align_of::<u64>());

        assert_eq!(size_of::<FourFieldsU128>(), size_of::<u128>());
        assert_eq!(align_of::<FourFieldsU128>(), align_of::<u128>());
    }

    #[bitfield_repr(u8)]
    pub enum CustomFieldRepr {
        Option1 = 0xa,
        Option2 = 0xf,
    }

    layout!({
        pub struct Example(u64);
        {
            let u32_repr @ 44..27;
            let custom @ 26..23: CustomFieldRepr;
            let custom_with_default @ 22..19: CustomFieldRepr =
                CustomFieldRepr::Option1;
            let __ @ 18..11 = 0xef;
            let with_default @ 10..9 = 0b11;
            let bit @ 8;
            let u8_repr @ 7..4;
            let __ @ 3..2 = 1;
            let __ @ 1..0;
        }
    });

    #[test]
    fn constants() {
        assert_eq!(Example::RSVD1_MASK, (0xef << 11) | (0b01 << 2));
        assert_eq!(Example::RSVD0_MASK, (0x10 << 11) | (0b10 << 2));

        assert_eq!(
            Example::DEFAULT,
            (0xef << 11)
                | (0b01 << 2)
                | ((CustomFieldRepr::Option1 as u64) << 19)
                | (0b11 << 9)
        );

        assert_eq!(Example::U32_REPR_MASK, 0x1ffff8000000);
        assert_eq!(Example::U32_REPR_SHIFT, 27usize,);

        assert_eq!(Example::CUSTOM_MASK, 0x7800000);
        assert_eq!(Example::CUSTOM_SHIFT, 23usize);

        assert_eq!(Example::CUSTOM_WITH_DEFAULT_MASK, 0x780000);
        assert_eq!(Example::CUSTOM_WITH_DEFAULT_SHIFT, 19usize);

        assert_eq!(Example::RSVD_18_11, 0xef << 11);

        assert_eq!(Example::WITH_DEFAULT_MASK, 0x600);
        assert_eq!(Example::WITH_DEFAULT_SHIFT, 9usize);

        assert_eq!(Example::BIT_SHIFT, 8usize);

        assert_eq!(Example::U8_REPR_MASK, 0xf0);
        assert_eq!(Example::U8_REPR_SHIFT, 4usize);

        assert_eq!(Example::RSVD_3_2, 1 << 2);
    }

    // new() should return a value with only reserved-as values set.
    #[test]
    fn new() {
        assert_eq!(*Example::new(), Example::RSVD1_MASK);
    }

    // default() should return a value with only defaults and reserved-as
    // values set.
    #[test]
    fn default() {
        assert_eq!(*Example::default(), Example::DEFAULT);
    }

    #[test]
    fn from() {
        assert_eq!(
            *Example::from(0 | Example::RSVD1_MASK),
            0 | Example::RSVD1_MASK
        );
        assert_eq!(
            *Example::from(1 | Example::RSVD1_MASK),
            1 | Example::RSVD1_MASK
        );
        assert_eq!(
            *Example::from(0xffff_0000_0000_0000 | Example::RSVD1_MASK),
            0xffff_0000_0000_0000 | Example::RSVD1_MASK
        );
    }

    #[test]
    fn derefmut_does_not_respect_reserved() {
        let mut example = Example::from(Example::RSVD1_MASK);
        *example &= !Example::RSVD1_MASK;
        assert_eq!(*example, 0);
    }

    #[test]
    fn from_then_get() {
        let example = Example::from(
            0xabcd << 27
                | (CustomFieldRepr::Option1 as u64) << 23
                | (CustomFieldRepr::Option2 as u64) << 19
                | 0b10 << 9
                | 1 << 8
                | 0xc << 4
                | Example::RSVD1_MASK,
        );
        assert_eq!(example.u32_repr(), 0xabcd);
        assert_eq!(example.custom().unwrap(), CustomFieldRepr::Option1);
        assert_eq!(
            example.custom_with_default().unwrap(),
            CustomFieldRepr::Option2
        );
        assert_eq!(example.with_default(), 0b10);
        assert!(example.bit());
        assert_eq!(example.u8_repr(), 0xc);
    }

    #[test]
    fn set_then_get() {
        let example = *Example::new()
            .set_u32_repr(0xabcd)
            .set_custom(CustomFieldRepr::Option1)
            .set_custom_with_default(CustomFieldRepr::Option2)
            .set_with_default(0b10)
            .set_bit(true)
            .set_u8_repr(0xc);
        assert_eq!(example.u32_repr(), 0xabcd);
        assert_eq!(example.custom().unwrap(), CustomFieldRepr::Option1);
        assert_eq!(
            example.custom_with_default().unwrap(),
            CustomFieldRepr::Option2
        );
        assert_eq!(example.with_default(), 0b10);
        assert!(example.bit());
        assert_eq!(example.u8_repr(), 0xc);
    }

    #[test]
    fn iter() {
        type Metadata = FieldMetadata<u64>;

        let example = *Example::new()
            .set_u32_repr(0xabcd)
            .set_custom(CustomFieldRepr::Option1)
            .set_custom_with_default(CustomFieldRepr::Option2)
            .set_with_default(0b10)
            .set_bit(true)
            .set_u8_repr(0xc);

        const EXPECTED: [(u64, Metadata); 6] = [
            (
                0xabcd,
                Metadata {
                    name: "u32_repr",
                    high_bit: 44,
                    low_bit: 27,
                    default: 0,
                },
            ),
            (
                0xa,
                Metadata {
                    name: "custom",
                    high_bit: 26,
                    low_bit: 23,
                    default: 0,
                },
            ),
            (
                0xf,
                Metadata {
                    name: "custom_with_default",
                    high_bit: 22,
                    low_bit: 19,
                    default: 0xa,
                },
            ),
            (
                0b10,
                Metadata {
                    name: "with_default",
                    high_bit: 10,
                    low_bit: 9,
                    default: 0b11,
                },
            ),
            (
                0b1,
                Metadata {
                    name: "bit",
                    high_bit: 8,
                    low_bit: 8,
                    default: 0,
                },
            ),
            (
                0xc,
                Metadata {
                    name: "u8_repr",
                    high_bit: 7,
                    low_bit: 4,
                    default: 0,
                },
            ),
        ];

        let actual: Vec<(&'static Metadata, u64)> =
            example.into_iter().collect();
        let rev_actual: Vec<(&'static Metadata, u64)> =
            example.into_iter().rev().collect();

        assert_eq!(actual.len(), EXPECTED.len());
        assert_eq!(rev_actual.len(), EXPECTED.len());
        for i in 0..EXPECTED.len() {
            let (expected_val, expected_metadata) = &EXPECTED[i];
            for (label, (actual_metadata, actual_val)) in [
                ("fwd", &actual[i]),
                ("rev", &rev_actual[EXPECTED.len() - 1 - i]),
            ] {
                assert_eq!(actual_val, expected_val, "{label}:{i}");
                assert_eq!(
                    actual_metadata.name, expected_metadata.name,
                    "{label}:{i}"
                );
                assert_eq!(
                    actual_metadata.high_bit, expected_metadata.high_bit,
                    "{label}:{i}"
                );
                assert_eq!(
                    actual_metadata.low_bit, expected_metadata.low_bit,
                    "{label}:{i}"
                );
                assert_eq!(
                    actual_metadata.default, expected_metadata.default,
                    "{label}:{i}"
                );
            }
        }
    }

    layout!({
        struct Unshifted(u32);
        {
            let field @ 19..16;
            #[unshifted]
            let unshifted_field @ 15..12;
            let __ @ 11..9;
            #[unshifted]
            let unshifted_bit @ 8;
            let normal_bit @ 7;
            let __ @ 6..0;
        }
    });

    #[test]
    fn unshifted_multi_bit_getter() {
        let val = Unshifted::from(0x5 << 12);
        assert_eq!(val.unshifted_field(), 0x5000);
    }

    #[test]
    fn unshifted_multi_bit_setter() {
        let mut val = Unshifted::new();
        val.set_unshifted_field(0xa000);
        assert_eq!(val.unshifted_field(), 0xa000);
        assert_eq!(*val & (0xf << 12), 0xa000);
    }

    #[test]
    fn unshifted_single_bit_getter() {
        let val = Unshifted::from(1 << 8);
        assert_eq!(val.unshifted_bit(), 1 << 8);

        let val = Unshifted::from(0);
        assert_eq!(val.unshifted_bit(), 0);
    }

    #[test]
    fn unshifted_single_bit_setter() {
        let mut val = Unshifted::new();
        val.set_unshifted_bit(1 << 8);
        assert_eq!(val.unshifted_bit(), 1 << 8);

        val.set_unshifted_bit(0);
        assert_eq!(val.unshifted_bit(), 0);
    }

    #[test]
    fn unshifted_round_trip() {
        let val = *Unshifted::new()
            .set_unshifted_field(0x7000)
            .set_unshifted_bit(1 << 8)
            .set_field(0xa)
            .set_normal_bit(true);
        assert_eq!(val.unshifted_field(), 0x7000);
        assert_eq!(val.unshifted_bit(), 1 << 8);
        assert_eq!(val.field(), 0xa);
        assert!(val.normal_bit());
    }

    #[test]
    fn unshifted_ignores_other_bits() {
        let val = Unshifted::from(0xffff_ffff);
        assert_eq!(val.unshifted_field(), 0xf000);
        assert_eq!(val.unshifted_bit(), 1 << 8);
    }
}
