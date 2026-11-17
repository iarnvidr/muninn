# The executable is attached by the Dagger image function from its shared Cargo
# cache. Docker never recompiles Rust or changes the native app's packaging.
FROM python:3.12-slim-bookworm@sha256:e697c81897302a4c4333bade000d98038fbb21eb064f8ee1fd42d02ebcd9d337
ENV PYTHONUNBUFFERED=1 PYTHONDONTWRITEBYTECODE=1 \
    PLAYWRIGHT_BROWSERS_PATH=/opt/chromium \
    MUNINN_DATA_DIR=/var/lib/muninn HOME=/var/lib/muninn
COPY requirements.txt /tmp/requirements.txt
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && pip install --no-cache-dir -r /tmp/requirements.txt \
    && patchright install --with-deps chromium \
    && rm -rf /var/lib/apt/lists/* /tmp/requirements.txt \
    && useradd --uid 10001 --create-home --home-dir /var/lib/muninn muninn
COPY . /opt/muninn-preview/
RUN cd /opt/muninn-preview && python3 -m unittest test_preview -v
USER 10001:10001
WORKDIR /opt/muninn-preview
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=4 CMD ["python3", "/opt/muninn-preview/health.py"]
ENTRYPOINT ["python3", "/opt/muninn-preview/gateway.py"]
