# SSRF allow-list

Restrict which URLs the [`http` step](../dsl/steps/http.md) can call.

## Configuration

```yaml
internal_requests:
  disabled:                false   # true = block ALL outbound HTTP
  allowed_urls:            []      # origin- or path-scoped entries; empty = any URL
  allowed_ips:             []      # bare-IP hosts; empty = any host
  block_private_networks:  true    # default-deny loopback / link-local / RFC-1918 / ULA
```

## Enforcement

Per outbound call, in order:

1. If `disabled: true` → reject with `outbound HTTP is disabled by internal_requests.disabled`. Runs at the very top of the request path — self-call short-circuits, `unix://` scheme URLs, and `unix_socket_map` alias URLs are all blocked.
2. If `allowed_urls` is non-empty AND no entry matches → reject with `url not in internal_requests.allowed_urls: <url>`. See **Allow-list matching** below.
3. If `allowed_ips` is non-empty AND the URL's host isn't a bare IP in the list → reject with `url host '<host>' not in internal_requests.allowed_ips`.
4. If `block_private_networks: true` AND both `allowed_urls` and `allowed_ips` are empty AND the URL's target resolves to a private / link-local / loopback range → reject with `outbound to private / link-local target '<host>' blocked ...`. Applies to TCP outbounds only; self-call short-circuits and UDS transports are unaffected.

An explicit entry in `allowed_urls` or `allowed_ips` opts a host back in for `block_private_networks`, so operators with a legitimate loopback sidecar just list it.

Rejection surfaces as an `http` step error → `500 Internal Server Error` to the caller with `{"error": "HTTP request rejected: ..."}`.

## Allow-list matching

`allowed_urls` entries come in two shapes:

- **Bare origin** — `http://api.example.com`, `https://api.example.com:8443`. Match requires exact `scheme://host:port` equality (default ports 80 / 443 are materialised on both sides). `http://api.example.com` does NOT admit `http://api.example.com.attacker.com/`.
- **Path-scoped** — anything with a path, query, or fragment after the origin (`https://api.example.com/v1/`, `https://api.example.com/v1?tok=X`). Match requires the origins to be equal AND the request URL to `starts_with` the entry AND the character following the entry to be a URL delimiter (`/`, `?`, `#`, `&`) or end-of-string. An entry `/v1` therefore does NOT admit `/v1anything`; `/v1/` DOES admit `/v1/legit`.

`allowed_ips` is exact host-string comparison — literal IP addresses only, no CIDR, no DNS.

## Private-network blocklist

When enabled (default), the following ranges are rejected before dispatch:

- IPv4: `127.0.0.0/8` (loopback), `169.254.0.0/16` (link-local, includes cloud metadata `169.254.169.254`), `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` (RFC 1918), `100.64.0.0/10` (carrier-grade NAT), `0.0.0.0`, `255.255.255.255`.
- IPv6: `::1` (loopback), `fe80::/10` (link-local), `fc00::/7` (unique-local), `::`, IPv4-mapped equivalents.

Hostname targets are resolved via `tokio::net::lookup_host`; a single private hit in the returned address set rejects the request.

### Known limitation: DNS-rebinding TOCTOU

Between `check_ssrf`'s hostname resolve and reqwest's own connect-time resolve, an attacker who controls the authoritative DNS for a target (or is behind an operator's own recursive resolver with a poisoned cache) can return different addresses to the two queries and slip a private target past the check. Realistic exposure is small (recursive resolvers cache) but the defence is incomplete — pinning the outbound connect to the pre-resolved IP is tracked in `tasks/backlog/063-pin-reqwest-connect-to-resolved-ip.md`.

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

- Self-call short-circuits (URLs that target Ruuter's own listeners) bypass `check_ssrf` for allowlist and private-network checks — they never touch TCP. The top-level `disabled` gate still applies.
- UDS transports (`unix://` scheme, `unix_socket_map` aliases) also bypass `check_ssrf` because there is no IP to check. The top-level `disabled` gate still applies.
- Outbound HTTP redirects are NOT followed transparently — the reqwest client is built with `redirect(Policy::none())`. A DSL that needs to chase a `Location` header issues a fresh `http.<verb>` call, which re-runs `check_ssrf` on the new target.
- No IP-CIDR support in `allowed_ips` / `allowed_urls`. Exact string / origin match only.
