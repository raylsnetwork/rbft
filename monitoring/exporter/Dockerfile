FROM python:3.11-slim

# Install system tools for debugging (ps, bash, curl)
RUN apt-get update && apt-get install -y --no-install-recommends \
    procps curl bash && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY collector.py .

# Data volume for SQLite
VOLUME ["/data"]

ENV RPC_URL=http://host.docker.internal:8545 \
    INSTANCE=v0 \
    EXPORTER_PORT=9102 \
    SCRAPE_INTERVAL=5 \
    BALANCE_EVERY_N=6 \
    DB_PATH=/data/state.sqlite3

EXPOSE 9102

CMD ["python", "-u", "collector.py"]

