# Ruuter-RS DSL Samples

Comprehensive examples demonstrating all Ruuter DSL features.

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

## DSL Features Demonstrated

- ✅ **Return Step**: Simple responses, status codes, headers
- ✅ **Assign Step**: Variable assignment, complex objects
- ✅ **HTTP Step**: GET, POST, PUT, DELETE with headers/body/query
- ✅ **Switch Step**: Conditional branching, validation
- ✅ **Log Step**: Logging with variable interpolation
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
