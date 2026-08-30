FROM alpine:3.22
RUN apk add --no-cache tinyproxy
RUN adduser -D -u 1000 -g '' de
COPY tinyproxy.conf /etc/tinyproxy/tinyproxy.conf
USER 1000:1000
ENTRYPOINT ["tinyproxy", "-d", "-c", "/etc/tinyproxy/tinyproxy.conf"]
