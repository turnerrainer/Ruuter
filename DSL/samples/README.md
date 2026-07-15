# Ruuter-on-Rust DSL Samples

Comprehensive examples demonstrating all Ruuter DSL features.

> For a tight, LLM-focused reference of every step, guard convention,
> config knob, and framework endpoint, see
> [`docs/DSL_REFERENCE.md`](../../docs/DSL_REFERENCE.md).

## New in 0.4.0

| Route | Demonstrates |
|---|---|
| `GET  /samples/things[/…]`                     | Path parameters — one DSL serves `/things`, `/things/{id}`, `/things/{id}/{sub}` (task 018) |
| `GET  /samples/vault/secret`                   | In-folder guard convention `.guard.yml` (task 019) |
| `POST /samples/ops/restart`                    | Folder-wide guard (stacking, back-compat) |
| `POST /samples/ops/inject-fault/trigger`       | Bespoke guard override via `declaration.override_ancestors` (task 020) |
| `GET  /samples/http/patch-request`             | `http.patch` step (task 023) |
| `POST /samples/advanced/iterate-batch`         | `iterate` step with `collect` / `into` |
| `POST /samples/idempotent-transfer`            | Framework Idempotency-Key handling (client sends header) |
| `DSL/samples/ruuter.yaml.example`              | Operator config with every knob |

## Basic Samples

### GET /samples/ping
Simple ping-pong response with custom status and headers.

### GET /samples/basic/hello
Minimal "Hello World" example.

### GET /samples/basic/status-codes
Custom HTTP status codes.

### GET /samples/basic/custom-headers
Response with custom headers.

## Variable Assignment

### GET /samples/variables/assign-simple
Basic variable assignment and usage.

### GET /samples/variables/incoming-params
Extract and use query parameters.
Example: `GET /samples/variables/incoming-params?id=123&name=John`

### POST /samples/variables/body-extraction
Extract data from POST body.

### GET /samples/variables/complex-object
Build complex nested objects.

## HTTP Steps

### GET /samples/http/simple-get
Simple HTTP GET request to external API.

### POST /samples/http/post-data
POST data to external API.

### GET /samples/http/with-headers
HTTP request with custom headers and authentication.

### POST /samples/http/chained-requests
Multiple HTTP requests chained together.

## Conditionals (Switch Step)

### GET /samples/conditionals/simple-switch
Age-based categorization.
Example: `GET /samples/conditionals/simple-switch?age=25`

### POST /samples/conditionals/validation
Input validation with error handling.

### GET /samples/conditionals/multiple-conditions
Complex multi-condition logic.
Example: `GET /samples/conditionals/multiple-conditions?role=admin&authenticated=true`

## JavaScript Evaluation

### GET /samples/javascript/math-operations
Mathematical operations with JavaScript.
Example: `GET /samples/javascript/math-operations?a=10&b=5`

### GET /samples/javascript/string-operations
String manipulation functions.
Example: `GET /samples/javascript/string-operations?text=Hello`

### GET /samples/javascript/date-time
Date and time operations.

### GET /samples/javascript/array-operations
Array manipulation and processing.

### POST /samples/javascript/json-manipulation
JSON object operations.

## Advanced Patterns

### GET /samples/advanced/step-chaining
Multi-step processing pipeline.

### POST /samples/advanced/multi-step-processing
Complex workflow with validation and enrichment.

### GET /samples/advanced/logging-demo
Logging at different stages.
Example: `GET /samples/advanced/logging-demo?userId=123`

### GET /samples/advanced/pagination
Pagination with offset/limit calculation.
Example: `GET /samples/advanced/pagination?page=2&size=10`

## Templates (Reusable DSLs)

### GET /samples/templates/user-profile
Reusable template for fetching user profile.

### POST /samples/templates/create-entity
Reusable template for entity creation with metadata.

### GET /samples/templates/call-template
Example of calling another DSL as a template.
Example: `GET /samples/templates/call-template?id=1&requester=admin`

### POST /samples/templates/call-create-template
Calling create template with validation.

**Template Syntax:**
```yaml
call_template:
  template: "project/METHOD/path/to/template"
  request_type: "GET" # or "POST", "PUT", "DELETE"
  body: {...}         # Optional
  query: {...}        # Optional
  headers: {...}      # Optional
  result: template_result
  next: next_step
```

## Guards (Authentication/Authorization)

### GET /samples/protected.guard.yml
Guard for all /samples/protected/* endpoints.
Requires Bearer token authentication.

### GET /samples/protected/data
Protected endpoint requiring authentication.
Example: `curl -H "Authorization: Bearer token123" http://localhost:8080/samples/protected/data`

### POST /samples/admin.guard.yml
Guard for all /samples/admin/* endpoints.
Requires admin role.

### POST /samples/admin/delete-user
Admin-only endpoint.
Example: `curl -X POST -H "Authorization: Bearer token" -H "x-user-role: admin" http://localhost:8080/samples/admin/delete-user`

### GET /samples/guards-demo
Explanation of how guards work.

**Guard Features:**
- Guards are DSL files with `.guard.yml` extension
- Guards execute **before** the main DSL
- If guard returns non-200 status, main DSL is blocked
- Guards are hierarchical - parent path guards apply to children
- Example: `GET/users.guard.yml` protects all `/users/*` endpoints

**Guard File Naming:**
```
DSL/
  project/
    GET/
      protected.guard.yml    # Guards /protected/*
      protected/
        data.yml             # Protected by guard
    POST/
      admin.guard.yml        # Guards /admin/*
      admin/
        delete-user.yml      # Protected by guard
```

## DSL Features Demonstrated

- ✅ **Return Step**: Simple responses, status codes, headers
- ✅ **Assign Step**: Variable assignment, complex objects
- ✅ **HTTP Step**: GET, POST, PUT, DELETE with headers/body/query
- ✅ **Switch Step**: Conditional branching, validation
- ✅ **Log Step**: Logging with variable interpolation
- ✅ **Template Step**: Recursive DSL calls, reusable components
- ✅ **Guards**: Authentication, authorization, access control
- ✅ **JavaScript**: Math, strings, dates, arrays, JSON
- ✅ **Variable Access**: incoming.params, incoming.body, incoming.headers
- ✅ **Step Chaining**: Sequential execution with next
- ✅ **Constants**: [#CONSTANT] syntax
- ✅ **Status Codes**: Custom HTTP status codes
- ✅ **Headers**: Custom response headers

## Running the Samples

```bash
# Start the server
docker-compose up -d

# Test basic endpoint
curl http://localhost:8080/samples/ping

# Test with parameters
curl "http://localhost:8080/samples/variables/incoming-params?id=123&name=John"

# Test POST
curl -X POST http://localhost:8080/samples/variables/body-extraction \
  -H "Content-Type: application/json" \
  -d '{"username":"john","email":"john@example.com"}'

# Test math operations
curl "http://localhost:8080/samples/javascript/math-operations?a=10&b=5"

# Test pagination
curl "http://localhost:8080/samples/advanced/pagination?page=2&size=10"

# Test template call
curl "http://localhost:8080/samples/templates/call-template?id=1&requester=admin"

# Test protected endpoint (will fail without token)
curl http://localhost:8080/samples/protected/data

# Test protected endpoint (with token)
curl -H "Authorization: Bearer my-secret-token-12345" \
  http://localhost:8080/samples/protected/data

# Test admin endpoint (requires role)
curl -X POST \
  -H "Authorization: Bearer token" \
  -H "x-user-role: admin" \
  -H "Content-Type: application/json" \
  -d '{"userId":"123"}' \
  http://localhost:8080/samples/admin/delete-user
```

## DSL Syntax Quick Reference

### Basic Structure
```yaml
step_name:
  return: "value"
  next: end
```

### Variable Assignment
```yaml
assign_step:
  assign:
    var_name: "value"
    number: 42
  next: next_step
```

### HTTP Request
```yaml
http_step:
  call: http.get
  args:
    url: "https://api.example.com/data"
    headers:
      Authorization: "Bearer token"
  result: response_var
  next: next_step
```

### Conditional
```yaml
check_step:
  switch:
    - condition: ${age >= 18}
      next: adult_step
    - condition: ${age >= 13}
      next: teen_step
  next: default_step
```

### Template Call
```yaml
template_step:
  template: "project/GET/path/to/template"
  request_type: "GET"
  query:
    param: ${value}
  result: template_result
  next: next_step
```

### Guard
```yaml
# In file: GET/protected.guard.yml
check_auth:
  switch:
    - condition: ${!incoming.headers.authorization}
      next: unauthorized
  next: authorized

authorized:
  status: 200
  return: "Guard passed"
  next: end

unauthorized:
  status: 401
  return: "Auth required"
  next: end
```

### JavaScript Expression
```yaml
calc_step:
  assign:
    result: ${Math.max(a, b) + 10}
  next: next_step
```

### Logging
```yaml
log_step:
  log: "Processing user: ${userId}"
  next: next_step
```

## Tips

1. **Variable Interpolation**: Use `${expression}` for JavaScript evaluation
2. **Incoming Data**: Access via `incoming.params`, `incoming.body`, `incoming.headers`
3. **Step Results**: Previous steps stored in variables: `${step_name.result}`
4. **Constants**: Use `[#CONSTANT_NAME]` for values from constants.ini
5. **Chaining**: Use `next:` to control flow, `end` to terminate
6. **Status Codes**: Set via `status:` in return step
7. **Headers**: Add via `headers:` in return step
8. **Templates**: Reuse DSLs with `template:` step for modularity
9. **Guards**: Protect endpoints with `.guard.yml` files
10. **Hierarchical Guards**: Parent guards apply to all child paths
