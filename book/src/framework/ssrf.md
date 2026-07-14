# SSRF allow-list

Restrict which URLs the [`http` step](../dsl/steps/http.md) can call.

## Configuration

```yaml
internal_requests:
  disabled:      false     # true = block ALL outbound HTTP
  allowed_urls:  []        # URL prefixes; empty = any URL
  allowed_ips:   []        # bare-IP hosts; empty = any host
```

## Enforcement

Per outbound call, in order:

1. If `disabled: true` → reject with `outbound HTTP is disabled by internal_requests.disabled`.
2. If `allowed_urls` is non-empty AND the URL doesn't start with any listed prefix → reject with `url not in internal_requests.allowed_urls: <url>`.
3. If `allowed_ips` is non-empty AND the URL's host isn't a bare IP in the list → reject with `url host '<host>' not in internal_requests.allowed_ips`.

Rejection surfaces as an `http` step error → `500 Internal Server Error` to the caller with `{"error": "HTTP request rejected: ..."}`.

## Allow-list matching

- `allowed_urls`: string prefix comparison. `https://api.example.com` matches `https://api.example.com/v1/orders` but NOT `https://api.example.com.attacker.com/`. Include a trailing slash if you want to lock the host boundary: `https://api.example.com/`.
- `allowed_ips`: exact host string comparison. Only URLs whose host component is one of these literal strings (no DNS resolution).

## Verification

```yaml
internal_requests:
  disabled: true
```

```
$ curl -sS -X GET http://localhost:8080/svc/that-calls-outbound
{"error":"HTTP request rejected: outbound HTTP is disabled by internal_requests.disabled"}
```

## Design notes

- No DNS resolution is performed to prevent DNS-rebinding surprises. Use `allowed_urls` for domain-based control; `allowed_ips` for locked-down egress environments.
- No IP-CIDR support (yet). Loopback / private-range blanket blocks are up to the operator's network layer.
