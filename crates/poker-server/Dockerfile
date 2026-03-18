FROM docker.io/library/debian:trixie-slim
COPY poker-server /usr/local/bin/poker-server
COPY static /srv/static
ENV POKER_STATIC_DIR=/srv/static
EXPOSE 3000
CMD ["poker-server"]
