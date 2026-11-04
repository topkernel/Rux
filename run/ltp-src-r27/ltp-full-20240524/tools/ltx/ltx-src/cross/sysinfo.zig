const std = @import("std");
const os = std.os;
const log = std.log;

pub fn main() anyerror!void {
    const name = os.uname();

    log.info("uname: {s} {s} {s} {s} {s} {s}", name);
}
