import socket, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
s.settimeout(2)
def rd():
    out = b""
    try:
        while True:
            b = s.recv(65536)
            if not b: break
            out += b
    except socket.timeout:
        pass
    return out.decode(errors="replace")
time.sleep(0.3)
print(rd()[-2000:])
for cmd in sys.argv[2:]:
    s.sendall((cmd + "\n").encode())
    time.sleep(0.7)
    print("=== " + cmd)
    print(rd()[-6000:])
