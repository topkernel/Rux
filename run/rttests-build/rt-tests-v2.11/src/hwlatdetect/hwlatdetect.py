#!/usr/bin/python3
""" Module to detect smis """

# SPDX-License-Identifier: GPL-2.0-only

# (C) 2018,2019 Clark Williams <williams@redhat.com>
# (C) 2015,2016 Clark Williams <williams@redhat.com>
# (C) 2009 Clark Williams <williams@redhat.com>

import abc
import errno
import os
import os.path
import subprocess
import sys
import time
from datetime import datetime

version = "0.8"
debugging = False
quiet = False
watch = False
args = None


def debug(dstr):
    """ print debugging string if debugging is on """
    if debugging:
        print(dstr)


def info(istr):
    " print info string if quiet is not on """
    if not quiet:
        print(istr)


def group_sep(num_hex):
    """ Add a comma as a grouping separator every 32 bits to a hexadecimal number. """
    return ','.join([num_hex[max(i - 8, 0):i] for i in range(len(num_hex), 0, -8)][::-1])


def key_values(fields):
    """ Extract key:value pairs from a list of fields and return them as a dictionary """
    return {
        k: v
        for field in fields
            if ':' in field
                for k, v in [field.split(":", 1)]
                    if k and v
    }


#
# Class used to manage mounting and umounting the debugfs
# filesystem. Note that if an instance of this class mounts
# the debugfs, it will unmount when cleaning up, but if it
# discovers that debugfs is already mounted, it will leave
# it mounted.
#
class DebugFS:
    """ class to manage mounting/umounting the debugfs """
    def __init__(self):
        self.premounted = False
        self.mounted = False
        self.mountpoint = ''
        with open('/proc/mounts') as f:
            for l in f:
                field = l.split()
                if field[2] == "debugfs":
                    self.premounted = True
                    self.mountpoint = field[1]
                    break

    def mount(self, path='/sys/kernel/debug'):
        """ mount debugfs unless already mounted """
        if self.premounted or self.mounted:
            debug("not mounting debugfs")
            return True
        debug(f"mounting debugfs at {path}")
        self.mountpoint = path
        cmd = ['/bin/mount', '-t', 'debugfs', 'none', path]
        self.mounted = (subprocess.call(cmd) == 0)
        if not self.mounted:
            raise RuntimeError("Failed to mount debugfs")
        return self.mounted

    def umount(self):
        """ unmount debugfs if not premounted """
        if self.premounted or not self.mounted:
            debug("not umounting debugfs")
            return True
        debug("umounting debugfs")
        cmd = ['/bin/umount', self.mountpoint]
        self.mounted = subprocess.call(cmd) != 0
        if self.mounted:
            raise RuntimeError("Failed to umount debugfs")
        return not self.mounted

    def getval(self, item, nonblocking=False):
        """ get value of mountpoint/item """
        path = os.path.join(self.mountpoint, item)
        if nonblocking is False:
            with open(path) as f:
                val = f.readline()
        else:
            f = os.fdopen(os.open(path, os.O_RDONLY | os.O_NONBLOCK), "r")
            try:
                val = f.readline()
            except OSError as e:
                print(f"errno: {e}")
                if e.errno == errno.EAGAIN:
                    val = None
                else:
                    raise
            f.close()
        return val

    def putval(self, item, value):
        """ write value to mountpoint/item """
        path = os.path.join(self.mountpoint, item)
        with open(path, "w") as f:
            f.write(str(value))
            f.flush()

    def getpath(self, item):
        """ get mountpoint/item """
        return os.path.join(self.mountpoint, item)


class DetectorNotAvailable(Exception):
    """ Exception class for when tracer is not available """

    def __init__(self, name, msg):
        self.args = (name, msg)
        self.name = name
        self.msg = msg


class Detector:
    """ base class for detector modules """

    def __init__(self):
        self.type = "unknown"
        if os.getuid() != 0:
            raise RuntimeError("Must be root")
        self.debugfs = DebugFS()
        if not self.debugfs.mount():
            raise RuntimeError("failed to mount debugfs")
        self.samples = []
        self.testduration = 30  # in seconds
        self.have_msr = False
        self.initsmi = []
        if os.path.exists('/usr/sbin/rdmsr'):
            self.have_msr = True
            self.initsmi = self.getsmicounts()
        self.dma_latency_handle = None

    def getsmicounts(self):
        counts = []
        if self.have_msr:
            p = subprocess.Popen(['/usr/sbin/rdmsr', '-a', '-d', '0x34'], stdout=subprocess.PIPE)
            p.wait()
            counts = [int(x.strip()) for x in p.stdout.readlines()]
        return counts

    # methods for preventing/enabling c-state transitions
    #
    # opening /dev/cpu_dma_latency and writing a 32-bit zero to that file will
    # prevent c-state transitions while the file descriptor is open.
    #
    # use c_states_off() to disable c-state transitions
    # use c_states_on() to close the file descriptor and re-enable c-states
    #
    def c_states_off(self):
        """ disable c-state transitions """
        if os.path.exists("/dev/cpu_dma_latency"):
            self.dma_latency_handle = os.open("/dev/cpu_dma_latency", os.O_WRONLY)
            os.write(self.dma_latency_handle, b'\x00\x00\x00\x00')
            debug("c-states disabled")

    def c_states_on(self):
        """ close the file descriptor and re-enable c-states """
        if self.dma_latency_handle:
            os.close(self.dma_latency_handle)
            debug("c-states enabled")

    @abc.abstractmethod
    def cleanup(self):
        ''' abstract cleanup method, must override '''

    @abc.abstractmethod
    def get(self, field):
        ''' get the value of a debugfs field '''

    @abc.abstractmethod
    def set(self, field, val):
        ''' set a value in a debugfs field '''

    @abc.abstractmethod
    def save(self, output=None):
        ''' save sample data to output '''

    @abc.abstractmethod
    def display(self):
        ''' output the sample data as a string '''

    def start(self):
        """ enable the detector """
        count = 0
        threshold = int(self.get("threshold"))
        self.c_states_off()
        debug(f"enabling detector module (threshold: {threshold})")
        self.set("enable", 1)
        while self.get("enable") == 0:
            debug("still disabled, retrying in a bit")
            count += 1
            time.sleep(0.1)
            debug(f"retrying enable of detector module ({count})")
            self.set("enable", 1)
        if self.get("threshold") != threshold:
            debug("start: threshold reset by start, fixing")
            self.set("threshold", threshold)
        debug(f"detector module enabled (threshold: {int(self.get('threshold'))})")

    def stop(self):
        """ disable the detector """
        count = 0
        debug("disabling detector module")
        self.set("enable", 0)
        while self.get("enable") == 1:
            debug("still enabled, retrying in a bit")
            count += 1
            time.sleep(0.1)
            debug(f"retrying disable of detector module({count})")
            self.set("enable", 0)
        self.c_states_on()
        debug("detector module disabled")

    @abc.abstractmethod
    def detect(self):
        ''' get detector output '''


class Tracer(Detector):
    """ Class to handle running the hwlat tracer module of ftrace """

    __field_translation = {
        'width'     : "hwlat_detector/width",
        'window'    : "hwlat_detector/window",
        'enable'    : "tracing_on",
        'threshold' : "tracing_thresh",
        'cpumask'   : "tracing_cpumask",
    }

    class Sample:
        'private class for tracer sample data'
        __slots__ = 'cpu', 'timestamp', 'delta', 'inner', 'outer', 'count'
        prev = float('nan')

        def __init__(self, line):
            fields = line.split()
            if len(fields) < 8:
                raise ValueError(f"Unexpected trace format: {line}")

            kv = key_values(fields)

            # Parse CPU number from either [NNN] or NNd.... format
            cpu_field = fields[1]
            if not cpu_field:
                raise ValueError(f"Empty CPU field in: {line}")

            if cpu_field[0] == '[':
                # nolatency-format: [007]
                self.cpu = int(cpu_field[1:-1])
            else:
                # latency-format: 13d....
                self.cpu = int(cpu_field[:-5])
            i, o = fields[6].split('/')
            ts = fields[7][3:]
            self.timestamp = str(ts)
            ts_float = float(ts)
            self.delta = ts_float - self.__class__.prev
            self.__class__.prev = ts_float
            self.inner = int(i)
            self.outer = int(o)
            self.count = int(kv["count"]) if "count" in kv else None

        def format_timestamp(self):
            """Format timestamp with %n (nanoseconds) support."""
            if not args.time_format:
                return self.timestamp
            p = self.timestamp.split('.')
            ns = p[1] if len(p) > 1 else '0' * 9
            t = datetime.fromtimestamp(float(f"{p[0]}.{ns[:6]}"))
            return t.strftime(args.time_format.replace('%n', ns))

        def __str__(self):
            s = f"ts: {self.format_timestamp()}"
            s += f", delta:{self.delta:.6f}, inner:{self.inner}, outer:{self.outer}, cpu:{self.cpu}"
            return s if self.count is None else s + f", count:{self.count}"

        def display(self):
            """ convert object to string and print """
            print(str(self))

        def largest(self):
            """ return largest value of inner or outer """
            return max(self.inner, self.outer)

    def translate(self, field):
        path = self.debugfs.getpath('tracing')
        if field not in Tracer.__field_translation:
            return os.path.join(path, field)
        return os.path.join(path, Tracer.__field_translation[field])

    def __init__(self):
        super().__init__()
        path = self.debugfs.getpath('tracing/hwlat_detector')
        if not os.path.exists(path):
            raise DetectorNotAvailable("hwlat", "hwlat tracer not available")
        self.type = "tracer"
        self.samples = []
        self.first = None
        self.last = None
        self.set("enable", 0)
        self.set('current_tracer', 'hwlat')

    def set(self, field, val):
        path = self.translate(field)
        self.debugfs.putval(path, str(val))

    def get(self, field):
        if field == "count":
            # Use new count field if available, otherwise fall back to sample count
            if self.samples and self.samples[0].count is not None:
                return sum(s.count for s in self.samples)
            return len(self.samples)
        if field == "max":
            max = 0
            for values in self.samples:
                s = int(values.largest())
                if s > max:
                    max = s
            return max
        return self.debugfs.getval(self.translate(field))

    def detect(self):
        self.samples = []
        testend = time.time() + self.testduration
        pollcnt = 0
        self.start()
        try:
            while time.time() < testend:
                pollcnt += 1
                val = self.get_sample()
                while val:
                    self.first = self.first or val.timestamp
                    self.last = val.timestamp
                    self.samples.append(val)
                    if watch:
                        val.display()
                    val = self.get_sample()
                time.sleep(0.1)
        except KeyboardInterrupt as e:
            print("interrupted")
        self.stop()
        return self.samples

    def get_sample(self):
        val = None
        line = self.debugfs.getval("tracing/trace_pipe", nonblocking=True)
        if line:
            val = self.Sample(line)
        return val

    def save(self, output=None):
        """ save samples """
        if output:
            with open(output, "w") as f:
                for s in self.samples:
                    f.write(f"{s}\n")
                print(f"report saved to {output} ({len(self.samples)} samples)")

    def display(self):
        for s in self.samples:
            s.display()

    def cleanup(self):
        self.set("tracing_on", "0")
        self.set("current_tracer", "nop")
        if not self.debugfs.umount():
            raise RuntimeError("Failed to unmount debugfs")


def seconds(sval):
    "convert input string to value in seconds"
    if sval.isdigit():
        return int(sval)
    if sval[-2].isalpha():
        raise RuntimeError(f"illegal suffix for seconds: '{sval[-2:-1]}'")
    if sval[-1:] == 's':
        return int(sval[0:-1])
    if sval[-1:] == 'm':
        return int(sval[0:-1]) * 60
    if sval[-1:] == 'h':
        return int(sval[0:-1]) * 3600
    if sval[-1:] == 'd':
        return int(sval[0:-1]) * 86400
    if sval[-1:] == 'w':
        return int(sval[0:-1]) * 86400 * 7
    raise RuntimeError(f"invalid input for seconds: '{sval}'")


def milliseconds(sval):
    "convert input string to millsecond value"
    if sval.isdigit():
        return int(sval)
    if sval[-2:] == 'ms':
        return int(sval[0:-2])
    if sval[-1] == 's':
        return int(sval[0:-2]) * 1000
    if sval[-1] == 'm':
        return int(sval[0:-1]) * 1000 * 60
    if sval[-1] == 'h':
        return int(sval[0:-1]) * 1000 * 60 * 60
    raise RuntimeError(f"invalid input for milliseconds: {sval}")


def microseconds(sval):
    "convert input string to microsecond value"
    if sval.isdigit():
        return int(sval)
    if sval[-2:] == 'ms':
        return int(sval[0:-2]) * 1000
    if sval[-2:] == 'us':
        return int(sval[0:-2])
    if sval[-1:] == 's':
        return int(sval[0:-1]) * 1000 * 1000
    raise RuntimeError(f"invalid input for microseconds: '{sval}'")


if __name__ == '__main__':
    from argparse import ArgumentParser

    parser = ArgumentParser()
    parser.add_argument("--duration", default=None,
                        dest="duration",
                        help="total time to test for hardware latency: <n>{smdw}")

    parser.add_argument("--threshold", default=None,
                        dest="threshold",
                        help="value above which is considered an hardware latency")

    parser.add_argument("--hardlimit", default=None,
                        dest="hardlimit",
                        help="value above which the test is considered to fail")

    parser.add_argument("--window", default=None,
                        dest="window",
                        help="time between samples")

    parser.add_argument("--width", default=None,
                        dest="width",
                        help="time to actually measure")

    parser.add_argument("--report", default=None,
                        dest="report",
                        help="filename for sample data")

    parser.add_argument("--cpu-list", default=None,
                        dest="cpulist",
                        help="the CPUs for hwlat thread to move across")

    parser.add_argument("--debug", action="store_true", default=False,
                        dest="debug",
                        help="turn on debugging prints")

    parser.add_argument("--quiet", action="store_true", default=False,
                        dest="quiet",
                        help="turn off all screen output")

    parser.add_argument("--watch", action="store_true", default=False,
                        dest="watch",
                        help="print sample data to stdout as it arrives")

    parser.add_argument("--time-format",
                        help="strftime format for timestamps (e.g. %%H:%%M:%%S.%%n)")

    args = parser.parse_args()

    # need these before creating detector instance
    if args.debug:
        debugging = True
        quiet = False
        debug("debugging prints turned on")

    if args.quiet:
        quiet = True
        debugging = False

    detect = Tracer()

    if args.threshold:
        t = microseconds(args.threshold)
        detect.set("threshold", t)
        debug(f"threshold set to {t}us")

    if args.hardlimit:
        hardlimit = microseconds(args.hardlimit)
    else:
        hardlimit = int(detect.get("threshold"))
    debug(f"hardlimit set to {hardlimit}us")

    if args.window:
        w = microseconds(args.window)
        width = w//2
        if w < int(detect.get("width")):
            debug(f"shrinking width to {width} for new window of {w}")
            detect.set("width", width)
        debug(f"window parameter = {w}")
        detect.set("window", w)
        debug(f"window for sampling set to {w}us")

    if args.width:
        w = microseconds(args.width)
        if w > int(detect.get("window")):
            debug(f"widening window to {w*2} for new width of {w}")
            detect.set("window", w*2)
        debug(f"width parameter = {w}")
        detect.set("width", w)
        debug(f"sample width set to {w}us")

    if args.duration:
        detect.testduration = seconds(args.duration)
    else:
        detect.testduration = 120  # 2 minutes
    debug(f"test duration is {detect.testduration}s")

    if args.watch:
        watch = True

    reportfile = args.report

    if args.cpulist:
        cpumask = 0
        for c in args.cpulist.split(','):
            l, r = 0, 0
            if '-' in c:
                l, r = map(int, c.split('-'))
            else:
                l, r = map(int, [c, c])
            for i in range(l, r + 1):
                cpumask |= (1 << i)
        debug(f"set tracing_cpumask to {cpumask:x}")
        detect.set("cpumask", group_sep(f"{cpumask:x}"))

    info(f"hwlatdetect:  test duration {detect.testduration} seconds")
    info(f"   detector: {detect.type}")
    info("   parameters:")
    info(f"        CPU list:          {args.cpulist}")
    info(f"        Latency threshold: {int(detect.get('threshold'))}us")
    info(f"        Sample window:      {int(detect.get('window'))}us")
    info(f"        Sample width:      {int(detect.get('width'))}us")
    info(f"     Non-sampling period:  {(int(detect.get('window')) - int(detect.get('width')))}us")
    info(f"        Output File:       {reportfile}")
    info("\nStarting test")

    detect.detect()

    info("test finished")

    max_latency = detect.get("max")
    if max_latency == 0:
        info("Max Latency: Below threshold")
    else:
        info(f"Max Latency: {int(max_latency)}us")

    info(f"Samples recorded: {len(detect.samples)}")

    exceeding = detect.get("count")
    info(f"Samples exceeding threshold: {exceeding}")

    if exceeding > 1:
        width = int(detect.get('width'))
        if width > 0:
            mtbf = (((float(detect.last) - float(detect.first)) * int(detect.get('window'))) / ((exceeding - 1) * width))
        info(f"MTBF: {mtbf:.3f} seconds")

    if detect.have_msr:
        finishsmi = detect.getsmicounts()
        total_smis = 0
        for i, count in enumerate(finishsmi):
            if count > detect.initsmi[i]:
                smis = count - detect.initsmi[i]
                total_smis += smis
                print(f"{smis} SMIs occured on cpu {i}")
        info(f"SMIs during run: {total_smis}")

    maxlatency = int(detect.get("max"))

    if reportfile:
        detect.save(reportfile)

    if not watch:
        detect.display()

    detect.cleanup()
    sys.exit(maxlatency > hardlimit)
