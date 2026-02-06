use core::fmt;
use crate::console;

pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                console::putchar(b'\r');
            }
            console::putchar(b);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let _ = write!(&mut $crate::print::Console, $($arg)*);
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut _console = $crate::print::Console;
        let _ = ::core::fmt::Write::write_fmt(&mut _console, ::core::format_args!($($arg)*)).ok();
        let _ = ::core::fmt::Write::write_str(&mut _console, "\n").ok();
    });
}

/// Debug println - only prints in debug mode
/// Note: Only works with string literals, not format arguments
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut _console = $crate::print::Console;
        let _ = ::core::fmt::Write::write_fmt(&mut _console, ::core::format_args!($($arg)*)).ok();
        let _ = ::core::fmt::Write::write_str(&mut _console, "\n").ok();
    });
}

/// Release version - debug_println does nothing
#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => ({
        // Empty in release mode
    });
}
