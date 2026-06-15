// Copyright (c) 2026 Joshua Seaton
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use bitfld::multilayout;

multilayout!({
    pub struct Mstatus32(u32);
    pub struct Mstatus64(u64);
    pub struct Sstatus32(u32);
    pub struct Sstatus64(u64);

    // SD sits at XLEN-1.
    #[variant(Mstatus32, Sstatus32)]
    {
        let sd @ 31;
    }
    #[variant(Mstatus64, Sstatus64)]
    {
        let sd @ 63;
    }

    // RV64 high half. MBE/SBE/SXL are M-mode only; UXL is visible to both.
    #[variant(Mstatus64)]
    {
        let mbe @ 37;
        let sbe @ 36;
        let sxl @ 35..34;
    }
    #[variant(Mstatus64, Sstatus64)]
    {
        let uxl @ 33..32;
    }

    // M-mode-only low-half fields.
    #[variant(Mstatus32, Mstatus64)]
    {
        let tsr @ 22;
        let tw @ 21;
        let tvm @ 20;
        let mprv @ 17;
        let mpp @ 12..11;
        let mpie @ 7;
        let mie @ 3;
    }

    // Shared low-half fields.
    {
        let mxr @ 19;
        let sum @ 18;
        let xs @ 16..15;
        let fs @ 14..13;
        let vs @ 10..9;
        let spp @ 8;
        let ube @ 6;
        let spie @ 5;
        let sie @ 1;
    }
});

fn main() {
    let m32 = *Mstatus32::new()
        .set_sd(true)
        .set_mpp(0b11)
        .set_mxr(true)
        .set_mie(true);
    println!("Mstatus32: {m32:#?}");

    let m64 = *Mstatus64::new()
        .set_sd(true)
        .set_sxl(0b10)
        .set_uxl(0b10)
        .set_mpp(0b11)
        .set_mxr(true)
        .set_mie(true);
    println!("Mstatus64: {m64:#?}");

    let s32 = *Sstatus32::new()
        .set_sd(true)
        .set_mxr(true)
        .set_sum(true)
        .set_spp(true)
        .set_sie(true);
    println!("Sstatus32: {s32:#?}");

    let s64 = *Sstatus64::new()
        .set_sd(true)
        .set_uxl(0b10)
        .set_mxr(true)
        .set_sum(true)
        .set_spp(true)
        .set_sie(true);
    println!("Sstatus64: {s64:#?}");
}
