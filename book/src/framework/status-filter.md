# Upstream status filter

Allow-list the HTTP statuses the [`http` step](../dsl/steps/http.md) will accept from an upstream.

## Configuration

```yaml
http_codes_allow_list: []            # empty = accept everything
```

## Enforcement

When non-empty, an upstream response whose status is NOT in the list is treated as an error:

```
{"error":"HTTP request rejected: upstream status 500 not in http_codes_allow_list"}
```

Common patterns:

```yaml
http_codes_allow_list: [200, 201, 202, 204]     # only accept 2xx (no 3xx redirects)
http_codes_allow_list: [200, 404]               # explicit "not found = valid outcome"
```

## Not for the response Ruuter itself sends

This filters what upstreams are allowed to return TO Ruuter. Ruuter's own response status is controlled by the DSL's `return.status`.
