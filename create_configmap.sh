#!/bin/bash
# This script creates a ConfigMap named 'rog-config' from the local file '/etc/rog/config.toml'.
# The ConfigMap will have a key 'config.toml' containing the content of the file.
# Ensure you have kubectl configured to point to the correct cluster before running this script.
kubectl create configmap rog-config --from-file=config.toml=/etc/rog/config.toml
