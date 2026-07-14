# Quickstart

## 1. Run

```bash
git clone <repo> ruuter-rs
cd ruuter-rs
docker compose up -d --build
```

Serves on `http://localhost:8080`.

## 2. Verify

```bash
curl http://localhost:8080/health
# {"service":"ruuter-rs","status":"ok","version":"0.4.0"}

curl http://localhost:8080/_/openapi.json | head -c 200
```

## 3. First DSL

```yaml
# DSL/myapp/GET/hello.yml
respond:
  return: { greeting: "hi ${incoming.params.name || 'world'}" }
  status: 200
  next: end
```

Reachable at `GET /myapp/hello?name=Ada`.

## 4. Reload

DSLs are read once at boot. Restart the container after changing files:

```bash
docker compose restart ruuter-rs
```

Hot reload is not implemented in 0.4.0. `docker compose restart` reloads the DSL tree in < 1 s on the sample corpus.
