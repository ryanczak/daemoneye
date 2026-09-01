FROM alpine@sha256:14358309a308569c32bdc37e2e0e9694be33a9d99e68afb0f5ff33cc1f695dce
RUN apk add --no-cache curl jq git python3 coreutils
RUN adduser -D -u 1000 -g '' de
WORKDIR /de/work
USER 1000:1000
