# CS2 dedicated server + Metamod:Source + CounterStrikeSharp + BotLab plugin.
# The game itself is not in the image: it lives in the shared install (CS2_BASE_DIR).
# Metamod and CounterStrikeSharp must match the CS2 build; pass the exact Linux release
# archives you have verified (see README "Worker image").
FROM docker.io/library/debian:bookworm-slim AS addons
ARG METAMOD_URL
ARG CSS_URL
RUN test -n "$METAMOD_URL" && test -n "$CSS_URL" \
    || (echo "build args METAMOD_URL and CSS_URL are required" >&2; exit 1)
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /out
RUN curl -fsSL "$METAMOD_URL" -o mm.tar.gz && tar -xzf mm.tar.gz && rm mm.tar.gz \
    && curl -fsSL "$CSS_URL" -o css.zip && unzip -q -o css.zip && rm css.zip \
    && test -d addons/metamod && test -d addons/counterstrikesharp

FROM docker.io/joedwards32/cs2:latest
COPY --from=addons /out/addons /botlab/addons
# Built by plugin/BotLab/build.sh
COPY plugin/BotLab/out/ /botlab/addons/counterstrikesharp/plugins/BotLab/
COPY docker/worker/cfg/ /botlab/cfg/
COPY docker/worker/botprofile.db /botlab/botprofile.db
COPY docker/worker/botprofile.vpk /botlab/botprofile.vpk
COPY docker/worker/worker-entry.sh /botlab/
# Workers run from the shared install and never update it; the cs2-install service does.
CMD ["bash", "/botlab/worker-entry.sh"]
