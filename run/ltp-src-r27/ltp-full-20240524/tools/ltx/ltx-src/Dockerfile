# build ltx
FROM alpine:latest AS builder
WORKDIR /root
RUN apk add build-base linux-headers git
RUN git clone https://github.com/acerv/ltx.git && make -C /root/ltx debug

# create small container
FROM alpine:latest
COPY --from=builder /root/ltx/ltx /ltx
ENTRYPOINT [ "/ltx" ]
