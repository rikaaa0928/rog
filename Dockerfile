# Use a slim base image
FROM debian:trixie-slim AS final

# TARGETPLATFORM is an automatic build argument provided by docker/build-push-action
# It will be something like linux/amd64, linux/arm64, etc.
ARG TARGETPLATFORM

# Create a directory for the binary
WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the pre-built binary from the build context into the image.
# The file path in the context must match the TARGETPLATFORM.
# e.g., copy release/linux/amd64/rog
COPY release/${TARGETPLATFORM}/rog .

# Grant execute permissions to the binary
RUN chmod +x /app/rog

# Set the entrypoint for the container
ENTRYPOINT ["/app/rog"]
