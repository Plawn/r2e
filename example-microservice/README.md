# example-microservice

Two independent R2E services communicating via HTTP, demonstrating:

- Two `[[bin]]` targets in one workspace member
- Shared types module as API contract
- `#[bean]` with `#[config("services.product.url")]` for service discovery
- `reqwest::Client` wrapper as a DI-managed service
- Cross-service validation (order creation validates product availability)
- Error handling for inter-service HTTP calls

## Running

Start both services (in separate terminals):

```bash
# Terminal 1: Product service (port 3001)
cargo run -p example-microservice --bin product-service

# Terminal 2: Order service (port 3002)
cargo run -p example-microservice --bin order-service
```

## Endpoints

### Product Service (port 3001)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/products` | List all products |
| GET | `/products/{id}` | Get product by ID |
| GET | `/products/{id}/availability` | Check product availability |
| GET | `/health` | Health check |

### Order Service (port 3002)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/orders` | List all orders |
| POST | `/orders` | Create order (validates product via HTTP) |
| GET | `/health` | Health check |

## Testing the flow

```bash
# List products
curl http://localhost:3001/products

# Check availability
curl http://localhost:3001/products/1/availability

# Create an order (validates product exists and is available)
curl -X POST http://localhost:3002/orders \
  -H "Content-Type: application/json" \
  -d '{"product_id": 1, "quantity": 2}'

# List orders
curl http://localhost:3002/orders
```
