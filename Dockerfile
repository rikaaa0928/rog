# Stage 1: Build the application
FROM rust:latest AS builder

# Install protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler

WORKDIR /usr/src/rog

# Copy the Cargo files and download dependencies
COPY Cargo.toml Cargo.lock ./
# Create a dummy main.rs to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
# Remove the dummy main.rs
RUN rm -rf src

# Copy the rest of the source code
COPY . .

# Build the application
RUN cargo build --release

# Stage 2: Create the final image
FROM debian:bullseye-slim

# Copy the built binary from the builder stage
COPY --from=builder /usr/src/rog/target/release/rog /usr/local/bin/rog

# Set the entrypoint
CMD ["rog"]
