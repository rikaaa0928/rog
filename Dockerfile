# Use a slim base image
FROM debian:bullseye-slim AS final

# TARGETPLATFORM is an automatic build argument provided by docker/build-push-action
# It will be something like linux/amd64, linux/arm64, etc.
ARG TARGETPLATFORM

# Create a directory for the binary
WORKDIR /app

# Copy the pre-built binary from the build context into the image.
# The file path in the context must match the TARGETPLATFORM.
# e.g., copy release/linux/amd64/rog
COPY release/${TARGETPLATFORM}/rog .

# Set the entrypoint for the container
ENTRYPOINT ["/app/rog"]
