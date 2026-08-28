FROM alpine:3.22
RUN apk add --no-cache curl jq git python3 coreutils
RUN adduser -D -u 1000 -g '' de
WORKDIR /de/work
USER 1000:1000
