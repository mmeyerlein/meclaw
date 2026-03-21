# MeClaw — Installation Guide

Three ways to run MeClaw, from easiest to most flexible.

---

## Option 1: Docker Compose (empfohlen)

Für alle mit Docker auf dem Rechner. Drei Befehle, fertig.

```bash
git clone https://github.com/mmeyerlein/meclaw.git
cd meclaw
cp config.example.sql config.sql    # API Keys eintragen
docker compose -f docker-compose.build.yml up -d
```

Fertig. MeClaw läuft auf:
- **PostgreSQL:** `localhost:5432` (User: postgres/postgres)
- **Admin UI:** `http://localhost:8080`

### config.sql anpassen

```sql
-- Telegram Bot (optional)
INSERT INTO meclaw.channels (id, name, type, config) VALUES (
    '00000000-0000-0000-0000-000000000001',
    'telegram-main', 'telegram',
    '{"bot_token": "DEIN_BOT_TOKEN", "chat_id": "DEINE_CHAT_ID"}'
) ON CONFLICT (id) DO UPDATE SET config = EXCLUDED.config;

-- LLM Provider (mindestens einen)
UPDATE meclaw.llm_providers SET api_key = 'DEIN_OPENROUTER_KEY' WHERE id = 'openrouter';
```

### Nützliche Befehle

```bash
docker compose -f docker-compose.build.yml logs -f    # Logs
docker compose -f docker-compose.build.yml down        # Stoppen
docker compose -f docker-compose.build.yml down -v     # Stoppen + Daten löschen
```

---

## Option 2: Portainer (Community Edition)

Portainer CE kann keine Images bauen — aber dein Docker-Host kann. Das Image wird per SSH auf dem Docker-Host gebaut, danach in Portainer als Stack deployed.

### Schritt 1: Image auf dem Docker-Host bauen

SSH auf den Server, auf dem der Portainer Agent (oder Portainer selbst) läuft:

```bash
ssh user@dein-docker-host

git clone https://github.com/mmeyerlein/meclaw.git /opt/meclaw
cd /opt/meclaw
docker build -t meclaw:latest .
```

Das dauert ca. 2 Minuten. Danach ist das Image `meclaw:latest` lokal verfügbar — Portainer sieht es sofort.

### Schritt 2: config.sql vorbereiten

Auf dem Docker-Host:

```bash
cp /opt/meclaw/config.example.sql /opt/meclaw/config.sql
nano /opt/meclaw/config.sql   # API Keys eintragen
```

### Schritt 3: Stack in Portainer anlegen

1. Portainer Web-UI öffnen
2. Den Endpoint wählen, auf dem du gebaut hast
3. **Stacks → Add stack**
4. Name: `meclaw`
5. **Web editor** — folgendes einfügen:

```yaml
services:
  meclaw:
    image: meclaw:latest
    container_name: meclaw
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
    ports:
      - "5432:5432"
      - "8080:8080"
    volumes:
      - meclaw-data:/var/lib/postgresql/data
      - /opt/meclaw/config.sql:/config/config.sql:ro
    restart: unless-stopped
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d meclaw"]
      interval: 10s
      timeout: 5s
      retries: 5

volumes:
  meclaw-data:
```

6. **Deploy the stack** klicken

Fertig! MeClaw läuft als Portainer-managed Stack.

### Update

Wenn es eine neue Version gibt:

```bash
ssh user@dein-docker-host
cd /opt/meclaw
git pull
docker build -t meclaw:latest .
```

Dann in Portainer: Stack → meclaw → **Redeploy** (mit "Re-pull image" Checkbox).

> ⚠️ Bei Schema-Änderungen: Container stoppen, Volume löschen, neu deployen. Oder Migration-Scripts aus dem Changelog anwenden.

---

## Option 3: Manuell (bestehendes PostgreSQL 17)

Für Leute die MeClaw in eine bestehende PostgreSQL-Installation integrieren wollen.

### Voraussetzungen

- PostgreSQL 17+
- Extensions installiert:
  - `age` (Apache AGE)
  - `pg_cron`
  - `pg_net`
  - `pgvector`
  - `pg_background`
  - `pg_search` (ParadeDB)
  - `plpython3u`

### postgresql.conf

```
shared_preload_libraries = 'pg_cron,pg_net,age,pg_search'
pg_net.database_name = 'meclaw'
cron.database_name = 'meclaw'
pg_background.max_workers = 256
```

### Installation

```bash
git clone https://github.com/mmeyerlein/meclaw.git
cd meclaw

createdb meclaw
psql -d meclaw -f install.sql

cp config.example.sql config.sql   # API Keys eintragen
psql -d meclaw -f config.sql
```

---

## Smoke Tests

Nach der Installation:

```sql
-- Schnelltest (69 Tests)
SELECT meclaw.run_smoke_tests();

-- Vollständig (108 Tests, inkl. Swarm + Context Pipeline)
SELECT meclaw.run_all_tests();
```

Erwartete Fehler ohne API Key:
- `llm_provider openrouter missing or no api_key` — kein API Key konfiguriert
- `no brain_events` — ohne LLM kein Datenfluss
- `llm_sentiment = neutral/0` — braucht LLM-Call

Sobald ein API Key in `config.sql` eingetragen ist, sollten alle Tests grün werden.

---

## Ports

| Port | Dienst |
|---|---|
| 5432 | PostgreSQL |
| 8080 | Admin Web UI |

## Daten

Alle Daten liegen im PostgreSQL Volume (`meclaw-data`). Backup:

```bash
docker exec meclaw pg_dump -U postgres meclaw > backup.sql
```

Restore:

```bash
docker exec -i meclaw psql -U postgres meclaw < backup.sql
```
