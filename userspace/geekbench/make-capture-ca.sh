#!/bin/bash
# make-capture-ca.sh — one-shot CA + server certificate for the Geekbench
# result-capture path (see build-rootfs.sh and gb-capture-server.py).
# Usage: make-capture-ca.sh <outdir>   (writes ca.pem, srv.pem, srv.key)
set -euo pipefail
D="${1:?usage: make-capture-ca.sh <outdir>}"
mkdir -p "$D"

cat > "$D/ca.cnf" <<'EOF'
[req]
distinguished_name=dn
x509_extensions=v3_ca
prompt=no
[dn]
CN=Rux Geekbench Capture CA
[v3_ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
EOF

cat > "$D/srv.cnf" <<'EOF'
[req]
distinguished_name=dn
req_extensions=v3_req
prompt=no
[dn]
CN=browser.geekbench.com
[v3_req]
subjectAltName=DNS:browser.geekbench.com,DNS:www.geekbench.com,DNS:geekbench.com,DNS:pb.geekbench.com
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
EOF

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$D/ca.key" -out "$D/ca.pem" -days 30 -config "$D/ca.cnf" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$D/srv.key" -out "$D/srv.csr" -config "$D/srv.cnf" 2>/dev/null
openssl x509 -req -in "$D/srv.csr" -CA "$D/ca.pem" -CAkey "$D/ca.key" -CAcreateserial -out "$D/srv.pem" -days 30 -extfile "$D/srv.cnf" -extensions v3_req 2>/dev/null
rm -f "$D/srv.csr" "$D/ca.cnf" "$D/srv.cnf"
echo "make-capture-ca: $D/{ca.pem,srv.pem,srv.key} — install ca.pem into the"
echo "  host trust store (update-ca-certificates) and pass it as GB_CAPTURE_CA"
echo "  to build-rootfs.sh; serve srv.pem/srv.key with gb-capture-server.py."
