//! Experimental, half baked support for interrupts on the esp32c6 LP core

use crate::pac::{
    self,
    generic::raw::R,
    lp_ana,
    lp_i2c0,
    lp_io,
    lp_timer::{self},
    lp_uart,
    pmu,
};
/// Enable machine mode and external interrupts
pub unsafe fn enable_interrupts() {
    // set pin 30 of mie to enable all external interrupts.
    // all external interrupts are routed to int 30.
    // see esp32c6 technical reference section 3.3.2 and https://github.com/espressif/esp-idf/blob/master/components/ulp/lp_core/lp_core/lp_core_interrupt.c
    let bits: usize = 1 << 30;
    unsafe {
        // for some reason, this register section seems to be masked off in the riscv
        // crate or at least it wasn't setting it for some reason.
        // that implementation is so macro heavy, i have no idea what's actually going
        // on. so inline asm
        core::arch::asm!("csrw mie, {}", in(reg) bits);
        // sets mie of mstatus csr
        riscv::interrupt::enable();
    };
}
/// Disable machine mode and external interrupts
pub unsafe fn disable_interrupts() {
    // see esp32c6 technical reference section 3.3.2 and https://github.com/espressif/esp-idf/blob/master/components/ulp/lp_core/lp_core/lp_core_interrupt.c
    let bits = 0;
    unsafe {
        core::arch::asm!("csrw mie, {}", in(reg) bits);
        riscv::interrupt::disable();
    };
}
/// Interrupt bits in LPPERI_LP_INTERRUPT_SOURCE
// see esp32c6 technical reference section 3.11, register 3.28
// while esp32c6_lp / pac already has an interrupt enum, its members and their values are incorrect
// i assume they are the interrupts for the HP core from the LP peripherals
#[repr(u8)]
#[derive(Default, Debug, PartialEq, Eq, Clone, Hash)]
pub enum LpInterrupts {
    /// IO interrupt bit
    IO   = 1,
    /// I2C interrupt bit
    I2C  = 2,
    /// UART interrupt bit
    UART = 4,
    /// RTC interrupt bit
    RTC  = 8,
    /// PMU interrupt bit
    PMU  = 32,
}
/// Interrupt handlers struct
#[derive(Default, Debug, PartialEq, Eq, Clone, Hash)]
pub struct InterruptHandlers {
    // for some reason with io, the STATUS reg has the masked interrupts, while STATUS_INT is for
    // masking them
    /// io interrupt handler
    // T.R. sec 7.16.4 reg 7.41
    pub io: Option<fn(R<lp_io::status::STATUS_SPEC>)>,
    // T.R sec 29.11 reg 29.24
    /// i2c interrupt handler
    pub i2c: Option<fn(R<lp_i2c0::int_st::INT_ST_SPEC>)>,
    // T.R sec 27.7.2 reg 27.41
    /// uart interrupt handler
    pub uart: Option<fn(R<lp_uart::int_st::INT_ST_SPEC>)>,
    // T.R. sec 12.8, sec 12.10.3 reg 12.77, sec 12.10.4 reg 12.88
    // apparently the brownout detector interrupt also goes through RTC
    // it would probably make sense to separate it out later
    /// rtc interrupt handler
    pub rtc: Option<
        fn(
            (
                R<lp_timer::lp_int_st::LP_INT_ST_SPEC>,
                R<lp_ana::lp_int_st::LP_INT_ST_SPEC>,
            ),
        ),
    >,
    // T.R. sec 12.8, sec 12.10.1 reg 12.50
    /// pmu interrupt handler
    pub pmu: Option<fn(R<pmu::lp_int_st::LP_INT_ST_SPEC>)>,
}

/// Specific interrupt handlers
// i'm not sure this is the best approach
// but i don't know how else you could modify them at runtime, since there's
// only one interrupt vector
pub static mut INTERRUPT_HANDLERS: InterruptHandlers = InterruptHandlers {
    io: None,
    i2c: None,
    uart: None,
    rtc: None,
    pmu: None,
};

// the ISR decorator can be replaced by a intermediate jump to a naked asm fn
// to manually save regs, restore them and return with mret after calling this
// function
// instead of relying on an unstable feature
#[unsafe(no_mangle)]
unsafe extern "riscv-interrupt-m" fn interrupt_handler() {
    // since there are no interrupt priority levels in the LP core
    // stealing is okay, unless the HP core is setting/clearing the LP core's
    // interrupts for some reason idk how one could make this sound
    // require the HP cpu to give up all related LP peripherials to load LP code?
    // i'm pretty sure LP core entry arguements just get transmuted anyways
    let source = unsafe {
        pac::LP_PERI::steal()
            .interrupt_source()
            .read()
            .lp_interrupt_source()
            .bits() as usize
    };
    // currently we clear all interrupt bits and let the individual handlers deal
    // with the details
    unsafe {
        if (source & LpInterrupts::PMU as usize) != 0 {
            let pmu = pac::PMU::steal();
            if let Some(pmu_handler) = INTERRUPT_HANDLERS.pmu {
                pmu_handler(pmu.lp_int_st().read());
            }
            // T.R. sec 12.10.1 reg 12.50
            pmu.lp_int_clr().write(|w| w.bits(0xfff0_0000));
        }
        if (source & LpInterrupts::RTC as usize) != 0 {
            let rtc = pac::LP_TIMER::steal();
            let ana = pac::LP_ANA::steal();
            if let Some(rtc_handler) = INTERRUPT_HANDLERS.rtc {
                // again, separating the timer and bod handlers would probably make sense
                rtc_handler((rtc.lp_int_st().read(), ana.lp_int_st().read()))
            }
            rtc.lp_int_clr().write(|w| {
                w.main_timer().clear_bit_by_one();
                w.main_timer_overflow().clear_bit_by_one()
            });
            ana.lp_int_clr().write(|w| w.bod_mode0().clear_bit_by_one())
        }
        if (source & LpInterrupts::UART as usize) != 0 {
            let uart = pac::LP_UART::steal();
            if let Some(uart_handler) = INTERRUPT_HANDLERS.uart {
                uart_handler(uart.int_st().read())
            }
            // T.R. sec 27.7.2 reg 27.43
            uart.int_clr().write(|w| w.bits(0x000c7fff));
        }
        if (source & LpInterrupts::I2C as usize) != 0 {
            let i2c = pac::LP_I2C0::steal();
            if let Some(i2c_handler) = INTERRUPT_HANDLERS.i2c {
                i2c_handler(i2c.int_st().read())
            }
            // T.R. sec 29.11 reg 29.22
            i2c.int_clr().write(|w| w.bits(0x0007ffff));
        }
        if (source & LpInterrupts::IO as usize) != 0 {
            let io = pac::LP_IO::steal();
            if let Some(io_handler) = INTERRUPT_HANDLERS.io {
                io_handler(io.status().read())
            }
            // T.R. sec 7.16.4 reg 7.43
            io.status_w1tc().write(|w| w.bits(0x000000ff));
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "riscv-interrupt-m" fn exception_handler() {}
