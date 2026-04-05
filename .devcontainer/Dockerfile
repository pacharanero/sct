FROM mcr.microsoft.com/devcontainers/rust:2-1-trixie

ARG WORKSPACE_FOLDER

RUN apt-get update \
    && apt-get install -y \
        pkg-config \
        libssl-dev \
        build-essential \
        sqlite3 \
        libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

RUN wget https://github.com/duckdb/duckdb/releases/download/v0.9.2/duckdb_cli-linux-amd64.zip -O /tmp/duckdb.zip && \
    unzip /tmp/duckdb.zip -d /usr/local/bin && \
    rm /tmp/duckdb.zip && \
    chmod +x /usr/local/bin/duckdb

RUN mkdir -p $WORKSPACE_FOLDER/data-volume \
    && chown -R vscode:vscode $WORKSPACE_FOLDER/data-volume
