//! Counts a 32 bit value at a known point in memory, writes PMU interrupt bits
//! to it and waits 5s, on pmu interrupts.
//!
//! When using the ESP32-C6's LP core, this address in memory is `0x5000_2000`.
//!
//! Make sure the LP RAM is cleared before loading the code.
//% CHIPS: esp32c6

#![no_std]
#![no_main]

use esp_lp_hal::{
    delay::Delay,
    interrupt::{INTERRUPT_HANDLERS, enable_interrupts},
    pac,
    prelude::*,
};
use panic_halt as _;

const ADDRESS: u32 = 0x5000_2000;

#[entry]
fn main() -> ! {
    let mut i: u32 = 0;

    let ptr = ADDRESS as *mut u32;

    // Register interrupt for PMU events
    unsafe {
        INTERRUPT_HANDLERS.pmu = Some(|pmu_reg| {
            (ADDRESS as *mut u32).write_volatile(pmu_reg.bits());
            Delay.delay_ms(5000);
        })
    }
    // We could get this from the HP core, on entry, but it wouldn't be able to send
    // interrupts :c
    let pmu = unsafe { pac::PMU::steal() };

    // Enables interrupts from LP wakeup sources and HP triggers
    pmu.lp_int_ena().write(|w| {
        w.lp_cpu_wakeup().set_bit();
        w.hp_sw_trigger().set_bit()
    });
    // Can be triggered by from HP cpu with:
    // peripherals
    // .PMU
    // .register_block()
    // .hp_lp_cpu_comm()
    // .write(|w| w.hp_trigger_lp().set_bit());

    // Enable interrupts
    unsafe { enable_interrupts() };

    loop {
        i = i.wrapping_add(1u32);
        unsafe {
            ptr.write_volatile(i);
        }

        Delay.delay_ms(1000);
    }
}
