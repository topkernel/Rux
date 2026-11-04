"""
Unittests for LTX service.
Tests created for python 3.7+
"""
import os
import time
import subprocess
import signal
import pytest
import msgpack

# keep values alligned with ltx
LTX_NONE = 0xffff
LTX_ERROR = 0xff
LTX_VERSION = 0x00
LTX_PING = 0x01
LTX_PONG = 0x02
LTX_GET_FILE = 0x03
LTX_SET_FILE = 0x04
LTX_ENV = 0x05
LTX_CWD = 0x06
LTX_EXEC = 0x07
LTX_RESULT = 0x08
LTX_LOG = 0x09
LTX_DATA = 0xa0
LTX_KILL = 0xa1
MAX_SLOTS = 128
ALL_SLOTS = MAX_SLOTS
MAX_ENVS = 16


class LTXHelper:
    """
    Helper class to send/receive message from LTX.
    """

    def __init__(self, proc) -> None:
        self._proc = proc
        self._buff = bytes()
        self._start_time = time.monotonic_ns()

    @property
    def proc(self):
        """
        LTX subprocess.
        """
        return self._proc

    def read(self):
        """
        Read some data from stdout.
        """
        return os.read(self._proc.stdout.fileno(), 1 << 21)

    def expect_exact(self, data):
        """
        Expect for an exact message when reading from stdout.
        """
        length = len(data)

        while len(self._buff) < length:
            self._buff += self.read()

        for i in range(length):
            if self._buff[i] == data[i]:
                continue

            raise ValueError(
                f"Expected {hex(data[i])}, "
                f"but got {hex(self._buff[i])} at {i} in "
                f"'{self._buff.hex(' ')}' / {self._buff}")

        self._buff = self._buff[length:]

    def expect_n_bytes(self, n):
        """
        Read n bytes from stdout.
        """
        while len(self._buff) < n:
            self._buff += self.read()

        self._buff = self._buff[n:]

    def unpack_next(self):
        """
        Unpack the next package using msgpack.
        """
        unpacker = msgpack.Unpacker()
        msg = None

        unpacker.feed(self._buff)

        while not msg:
            try:
                msg = unpacker.unpack()
            except msgpack.OutOfData:
                data = self.read()
                self._buff += data
                unpacker.feed(data)

        self._buff = self._buff[unpacker.tell():]

        return msg

    def send(self, data, check_echo=True):
        """
        Send some data to stdin.
        """
        assert os.write(self._proc.stdin.fileno(), data) == len(data)

        if check_echo:
            self.expect_exact(data)


class TestLTX:
    """
    Test the LTX process.
    """

    @pytest.fixture(autouse=True, scope="session")
    def build_ltx(self):
        """
        Automatically build ltx service.
        """
        subprocess.call(["make", "debug"])
        yield
        subprocess.call(["make", "clean"])

    @pytest.fixture
    def ltx(self):
        """
        LTX service communication object.
        """
        parent = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        proc = subprocess.Popen(
            "./ltx",
            cwd=parent,
            bufsize=0,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE)

        yield proc

        proc.send_signal(signal.SIGINT)

    @pytest.fixture
    def ltx_helper(self, ltx):
        """
        Helper object for LTX process.
        """
        yield LTXHelper(ltx)

    def test_version(self, ltx_helper):
        """
        Test VERSION command.
        """
        ltx_helper.send(msgpack.packb([LTX_VERSION]), check_echo=False)
        ltx_helper.expect_exact(msgpack.packb([LTX_VERSION, "0.1"]))

    def test_ping(self, ltx_helper):
        """
        Test PING command.
        """
        start_t = time.monotonic_ns()

        ltx_helper.send(msgpack.packb([LTX_PING]))

        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_PONG
        assert start_t < reply[1] < time.monotonic_ns()

    def test_pong_error(self, ltx_helper):
        """
        Test that PONG command raises an ERROR:
        """
        ltx_helper.send(msgpack.packb([LTX_PONG]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "PONG should not be received" in reply[1]

    def test_error(self, ltx_helper):
        """
        Test that ERROR command raises an ERROR:
        """
        ltx_helper.send(msgpack.packb([LTX_ERROR]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "ERROR should not be received" in reply[1]

    def test_data_error(self, ltx_helper):
        """
        Test that DATA command raises an ERROR:
        """
        ltx_helper.send(msgpack.packb([LTX_DATA]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "DATA should not be received" in reply[1]

    def test_get_file(self, ltx_helper, tmpdir):
        """
        Test GET_FILE command.
        """
        path = tmpdir / "temp.bin"
        path_str = str(path)
        path.write(b'a' * 1024 + b'b' * 1024 + b'c' * 128)

        ltx_helper.send(msgpack.packb(
            [LTX_GET_FILE, path_str]), check_echo=False)

        ltx_helper.expect_exact(msgpack.packb([LTX_DATA, b'a' * 1024]))
        ltx_helper.expect_exact(msgpack.packb([LTX_DATA, b'b' * 1024]))
        ltx_helper.expect_exact(msgpack.packb([LTX_DATA, b'c' * 128]))
        ltx_helper.expect_exact(msgpack.packb([LTX_GET_FILE, path_str]))

    def test_get_file_from_proc(self, ltx_helper):
        """
        Test GET_FILE command reading from /proc.
        """
        path_str = "/proc/self/personality"

        ltx_helper.send(msgpack.packb(
            [LTX_GET_FILE, path_str]), check_echo=False)

        reply = ltx_helper.unpack_next()
        int(reply[1].rstrip(), 16)

        ltx_helper.expect_exact(msgpack.packb([LTX_GET_FILE, path_str]))

    def test_get_file_empty_path_error(self, ltx_helper):
        """
        Test GET_FILE command error when empty path is given.
        """
        ltx_helper.send(msgpack.packb([LTX_GET_FILE, '']), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Empty given path" in reply[1]

    def test_get_file_not_file_error(self, ltx_helper):
        """
        Test GET_FILE command error when regular file is not given.
        """
        ltx_helper.send(msgpack.packb([LTX_GET_FILE, "/"]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Given path is not a file" in reply[1]

    def test_set_file(self, ltx_helper, tmpdir):
        """
        Test SET_FILE command.
        """
        path_str = str(tmpdir / "temp.bin")
        data = b'ciao'

        ltx_helper.send(msgpack.packb(
            [LTX_SET_FILE, path_str, data]),
            check_echo=False)
        ltx_helper.expect_exact(msgpack.packb([LTX_SET_FILE, path_str]))

        assert os.path.isfile(path_str)

    def test_set_file_empty_path_error(self, ltx_helper):
        """
        Test SET_FILE command error when empty path is given.
        """
        ltx_helper.send(msgpack.packb(
            [LTX_SET_FILE, '', b'']), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Empty given path" in reply[1]

    def test_env_local(self, ltx_helper):
        """
        Test ENV command on single slot.
        """
        for i in range(0, MAX_ENVS):
            ltx_helper.send(msgpack.packb(
                [LTX_ENV, i, f"mykey{i}", f"myvalue{i}"]))

    def test_env_global(self, ltx_helper):
        """
        Test ENV command for all slots.
        """
        ltx_helper.send(msgpack.packb(
            [LTX_ENV, ALL_SLOTS, "mykey", "myvalue"]))

    def test_env_out_of_bound_error(self, ltx_helper):
        """
        Test ENV command on out-of-bound slot.
        """
        cmd = msgpack.packb([LTX_ENV, MAX_SLOTS + 1, "mykey", "myvalue"])

        ltx_helper.send(cmd, check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Out of bound slot ID" in reply[1]

    def test_env_too_many_error(self, ltx_helper):
        """
        Test ENV command when saturating the number of environment variables.
        """
        # saturate the amount of env variables
        for i in range(0, MAX_ENVS):
            ltx_helper.send(msgpack.packb(
                [LTX_ENV, 0, f"mykey{i}", f"myvalue{i}"]))

        # add just one more key and check for errors
        key = "mykey" + str(MAX_ENVS + 1)
        value = "myvalue" + str(MAX_ENVS + 1)

        ltx_helper.send(msgpack.packb(
            [LTX_ENV, 0, key, value]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Set too many environment variables" in reply[1]

    def test_cwd_local(self, ltx_helper, tmpdir):
        """
        Test CWD command on single slot.
        """
        for i in range(0, MAX_SLOTS):
            ltx_helper.send(msgpack.packb([LTX_CWD, i, str(tmpdir)]))

    def test_cwd_global(self, ltx_helper, tmpdir):
        """
        Test CWD command on single slot.
        """
        ltx_helper.send(msgpack.packb([LTX_CWD, ALL_SLOTS, str(tmpdir)]))

    def test_cwd_out_of_bound_error(self, ltx_helper, tmpdir):
        """
        Test CWD command on out-of-bound slot.
        """
        ltx_helper.send(msgpack.packb(
            [LTX_CWD, MAX_SLOTS + 1, str(tmpdir)]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Out of bound slot ID" in reply[1]

    def test_cwd_dir_does_not_exist_error(self, ltx_helper):
        """
        Test CWD command with non-existing directory.
        """
        ltx_helper.send(msgpack.packb(
            [LTX_CWD, 0, "/this/dir/doesnt/exist"]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "CWD directory does not exist" in reply[1]

    def test_exec(self, ltx_helper):
        """
        Test EXEC command on single slot.
        """
        slot = 0
        start_t = time.monotonic_ns()

        # run command
        ltx_helper.send(msgpack.packb([LTX_EXEC, slot, "uname"]))

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == 'Linux\n'

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_big_log(self, ltx_helper):
        """
        Test EXEC command on single slot generating a big stdout.
        """
        slot = 0
        start_t = time.monotonic_ns()

        # run command
        data = "x"*2048
        ltx_helper.send(msgpack.packb([LTX_EXEC, slot, f"echo -n {data}"]))

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == "x"*1024

        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == "x"*1024

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_multiple(self, ltx_helper):
        """
        Test EXEC command on multiple slots.
        """
        start_t = time.monotonic_ns()

        # run command. We add a little delay before command,
        # so we avoid to obtain LOG when EXEC echo is sent
        for slot in range(0, ALL_SLOTS):
            ltx_helper.send(msgpack.packb([
                LTX_EXEC,
                slot,
                "sleep 0.2 && uname"
            ]))

        # read LOG + RESULT for each EXEC
        for _ in range(0, 2 * MAX_SLOTS):
            reply = ltx_helper.unpack_next()

            assert reply[0] in (LTX_RESULT, LTX_LOG)
            assert reply[1] in range(0, MAX_SLOTS)
            assert start_t < reply[2] < time.monotonic_ns()

            if reply[0] == LTX_RESULT:
                assert reply[3] == os.CLD_EXITED
                assert reply[4] == 0
            elif reply[0] == LTX_LOG:
                assert reply[3] == 'Linux\n'

    def test_exec_out_of_bound_error(self, ltx_helper):
        """
        Test EXEC command on out-of-bounds slot.
        """
        ltx_helper.send(msgpack.packb(
            [LTX_EXEC, MAX_SLOTS + 1, "test"]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Out of bound slot ID" in reply[1]

    def test_exec_reserved_error(self, ltx_helper):
        """
        Test EXEC command when the same slot is used two times.
        """
        cmd = bytes()
        cmd += msgpack.packb([LTX_EXEC, 0, "sleep 0.3"])
        cmd += msgpack.packb([LTX_EXEC, 0, "echo ciao"])

        ltx_helper.send(cmd)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Execution slot is reserved" in reply[1]

    def test_exec_env_local(self, ltx_helper):
        """
        Test EXEC command after setting environ variable in a single slot.
        """
        start_t = time.monotonic_ns()
        slot = 0

        cmd = bytes()
        cmd += msgpack.packb([LTX_ENV, slot, "MYKEY", "MYVAL"])
        cmd += msgpack.packb([LTX_EXEC, slot, "echo -n $MYKEY"])

        ltx_helper.send(cmd)

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == 'MYVAL'

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_env_local_reset(self, ltx_helper):
        """
        Test EXEC command after setting environ variable in a single slot, then
        reset it and check if variable is still defined.
        """
        start_t = time.monotonic_ns()
        slot = 0

        cmd = bytes()
        cmd += msgpack.packb([LTX_ENV, slot, "MYKEY", "MYVAL"])
        cmd += msgpack.packb([LTX_ENV, slot, "MYKEY", ""])
        cmd += msgpack.packb([LTX_EXEC, slot, "echo -n $MYKEY"])

        ltx_helper.send(cmd)

        # no logs -> no LOG -> only result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_env_global(self, ltx_helper):
        """
        Test EXEC command after setting global environ variable.
        """
        start_t = time.monotonic_ns()
        slot = 0

        cmd = bytes()
        cmd += msgpack.packb([LTX_ENV, ALL_SLOTS, "MYKEY", "MYVAL"])
        cmd += msgpack.packb([LTX_EXEC, slot, "echo -n $MYKEY"])

        ltx_helper.send(cmd)

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == 'MYVAL'

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_cwd_local(self, ltx_helper, tmpdir):
        """
        Test EXEC command after setting current working directory in one slot.
        """
        start_t = time.monotonic_ns()
        slot = 0

        cmd = bytes()
        cmd += msgpack.packb([LTX_CWD, slot, str(tmpdir)])
        cmd += msgpack.packb([LTX_EXEC, slot, "echo -n $PWD"])

        ltx_helper.send(cmd)

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == str(tmpdir)

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_exec_cwd_global(self, ltx_helper, tmpdir):
        """
        Test EXEC command after setting current working directory in one slot.
        """
        start_t = time.monotonic_ns()
        slot = 0

        cmd = bytes()
        cmd += msgpack.packb([LTX_CWD, ALL_SLOTS, str(tmpdir)])
        cmd += msgpack.packb([LTX_EXEC, slot, "echo -n $PWD"])

        ltx_helper.send(cmd)

        # read logs
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_LOG
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == str(tmpdir)

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_EXITED
        assert reply[4] == 0

    def test_kill(self, ltx_helper):
        """
        Test KILL command on single slot.
        """
        slot = 0
        start_t = time.monotonic_ns()

        # run command
        ltx_helper.send(msgpack.packb([LTX_EXEC, slot, "sleep 3"]))

        # kill command
        ltx_helper.send(msgpack.packb([LTX_KILL, slot]))

        # read result
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_RESULT
        assert reply[1] == slot
        assert start_t < reply[2] < time.monotonic_ns()
        assert reply[3] == os.CLD_KILLED
        assert reply[4] == signal.SIGKILL

    def test_kill_out_of_bound_error(self, ltx_helper):
        """
        Test KILL command with out-of-bound slot.
        """
        ltx_helper.send(msgpack.packb([LTX_KILL, MAX_SLOTS]), check_echo=False)
        reply = ltx_helper.unpack_next()
        assert reply[0] == LTX_ERROR
        assert "Out of bound slot ID" in reply[1]
