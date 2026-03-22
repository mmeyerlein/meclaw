FROM postgres:17-bookworm

ARG PG_MAJOR=17

# Install build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    git \
    curl \
    ca-certificates \
    gnupg \
    postgresql-server-dev-${PG_MAJOR} \
    python3 \
    python3-dev \
    libpython3-dev \
    python3-psycopg2 \
    python3-requests \
    libcurl4-openssl-dev \
    libkrb5-dev \
    && rm -rf /var/lib/apt/lists/*

# Add PGDG repo for pre-built extensions
RUN curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc | gpg --dearmor -o /etc/apt/trusted.gpg.d/postgresql.gpg \
    && echo "deb http://apt.postgresql.org/pub/repos/apt bookworm-pgdg main" > /etc/apt/sources.list.d/pgdg.list

# Install pre-built extensions from PGDG
RUN apt-get update && apt-get install -y --no-install-recommends \
    postgresql-${PG_MAJOR}-pgvector \
    postgresql-${PG_MAJOR}-cron \
    postgresql-${PG_MAJOR}-age \
    postgresql-plpython3-${PG_MAJOR} \
    && rm -rf /var/lib/apt/lists/*

# pg_net: compile from source (v0.20.2, disable Werror for libcurl compat)
RUN git clone --depth 1 --branch v0.20.2 https://github.com/supabase/pg_net.git /tmp/pg_net \
    && cd /tmp/pg_net \
    && sed -i 's/-Werror//' Makefile \
    && make && make install \
    && rm -rf /tmp/pg_net

# pg_background: compile from source
RUN git clone --depth 1 https://github.com/vibhorkum/pg_background.git /tmp/pg_background \
    && cd /tmp/pg_background && make && make install && rm -rf /tmp/pg_background

# pg_search (ParadeDB BM25): install pre-built .deb v0.22.2
# Upgraded from v0.15.10 to fix rt_fetch out-of-bounds bug (GitHub #2462, #3135)
# Breaking changes since v0.19.0: paradedb.score → pdb.score
RUN curl -fsSL -o /tmp/pg_search.deb \
    https://github.com/paradedb/paradedb/releases/download/v0.22.2/postgresql-17-pg-search_0.22.2-1PARADEDB-bookworm_amd64.deb \
    && apt-get install -y /tmp/pg_search.deb \
    && rm /tmp/pg_search.deb

# PostgreSQL config
RUN echo "shared_preload_libraries = 'pg_cron,pg_net,age,pg_search'" >> /usr/share/postgresql/${PG_MAJOR}/postgresql.conf.sample \
    && echo "pg_net.database_name = 'meclaw'" >> /usr/share/postgresql/${PG_MAJOR}/postgresql.conf.sample \
    && echo "cron.database_name = 'meclaw'" >> /usr/share/postgresql/${PG_MAJOR}/postgresql.conf.sample \
    && echo "pg_background.max_workers = 256" >> /usr/share/postgresql/${PG_MAJOR}/postgresql.conf.sample

# Copy meclaw files
COPY sql/ /docker-entrypoint-initdb.d/meclaw/sql/
COPY install.sql /docker-entrypoint-initdb.d/meclaw/
COPY config.example.sql /docker-entrypoint-initdb.d/meclaw/
COPY docker/init-meclaw.sh /docker-entrypoint-initdb.d/00-init-meclaw.sh
RUN chmod +x /docker-entrypoint-initdb.d/00-init-meclaw.sh

EXPOSE 5432 8080
