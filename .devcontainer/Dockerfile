FROM mcr.microsoft.com/devcontainers/rust:2-1-trixie

ARG WORKSPACE_FOLDER
ARG DUCKDB_VERSION=v1.5.1

RUN apt-get update \
    && apt-get install -y \
        wget \
        curl \
        unzip \
        pkg-config \
        libssl-dev \
        build-essential \
        jq \
        ripgrep \
        sqlite3 \
        libsqlite3-dev \
        python3 \
        python3-pip \
        python3-venv \
    && rm -rf /var/lib/apt/lists/*

RUN wget https://github.com/duckdb/duckdb/releases/download/${DUCKDB_VERSION}/duckdb_cli-linux-amd64.zip -O /tmp/duckdb.zip && \
    unzip /tmp/duckdb.zip -d /usr/local/bin && \
    rm /tmp/duckdb.zip && \
    chmod +x /usr/local/bin/duckdb

# Ollama — used by `sct embed` and `sct semantic` for vector embeddings
RUN curl -fsSL https://ollama.com/install.sh | sh

RUN mkdir -p $WORKSPACE_FOLDER/data-volume \
    && chown -R vscode:vscode $WORKSPACE_FOLDER/data-volume
