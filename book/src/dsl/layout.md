# File layout

```
DSL/
  <project>/
    GET/    <path>.yml            → GET /<project>/<path>
    POST/   <path>.yml            → POST /<project>/<path>
    PUT/    ...
    PATCH/  ...
    DELETE/ ...
    OPTIONS/...
    WS/     <path>.yml            → ws://.../<project>/<path>
    triggers/<channel>/<key>.yml  → dispatched from a source (not routed)
    sources/<name>.yml            → outbound WS source config (not routed)
    cronmanager-jobs/*.yaml       → external CronManager configs (not routed)
    <stem>.guard.yml              → guards <METHOD>/<stem>/*
    <dir>/.guard.yml              → guards everything under <dir>/ (Java parity)
```

- `<project>` = first URL segment, chosen freely per app.
- `<path>` = the remaining URL segments (may nest).
- Nested folders are allowed under any HTTP-method directory:
  `POST/orders/create.yml` → `POST /<project>/orders/create`.
- All DSL files are `.yml` or `.yaml`. Guards can also be extension-less `.guard`.

## Reserved subdirectory names

`triggers`, `sources`, `cronmanager-jobs`, `WS`, and every valid HTTP method name are reserved. Any other subdirectory of `<project>/` is treated as an HTTP method — usually a mistake.

## Route resolution

1. Exact `<METHOD>/<path>` match wins.
2. On miss: strip trailing path segments one at a time. Each stripped segment prepends to `incoming.params.pathParams`. See [Path parameters](./path-params.md).
3. If no shortened key matches: `404 Not Found`.
