#!/bin/bash
# ==============================================================================
# rog TPROXY Teardown Script
# Usage: ./tproxy-down.sh [TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]
# ==============================================================================

TPROXY_PORT=${1:-${ROG_PORT:-12345}}
FWMARK=${2:-${ROG_FWMARK:-1}}
TABLE_ID=${3:-${ROG_TABLE:-100}}
FAMILY=${5:-${ROG_FAMILY:-auto}}

FAMILY=$(echo "$FAMILY" | tr '[:upper:]' '[:lower:]')

CHAIN_NAME="ROG_TPROXY"
CHAIN_NAME6="ROG_TPROXY6"

FWMARK_HEX=$(printf "0x%x" "$FWMARK" 2>/dev/null || echo "$FWMARK")
RULE_PATTERN="fwmark +(${FWMARK}|${FWMARK_HEX})(/0x[0-9a-fA-F]+)?\b.*lookup +${TABLE_ID}\b"

echo "[rog-tproxy] Cleaning up TPROXY rules (port: $TPROXY_PORT, table: $TABLE_ID)..."

# 1. Clean up IPv4
if [ "$FAMILY" != "v6" ] && [ "$FAMILY" != "ipv6" ]; then
    while iptables -t mangle -D PREROUTING -j "$CHAIN_NAME" 2>/dev/null; do :; done
    iptables -t mangle -F "$CHAIN_NAME" 2>/dev/null || true
    iptables -t mangle -X "$CHAIN_NAME" 2>/dev/null || true

    # Delete the fwmark policy rule
    while ip -4 rule show | grep -qE "$RULE_PATTERN"; do
        ip -4 rule del fwmark "$FWMARK" table "$TABLE_ID" 2>/dev/null || break
    done

    # If no other rules point to this table, flush the table
    if ! ip -4 rule show | grep -qE "lookup +${TABLE_ID}\b"; then
        ip -4 route flush table "$TABLE_ID" 2>/dev/null || true
    fi
fi

# 2. Clean up IPv6
if [ "$FAMILY" != "v4" ] && [ "$FAMILY" != "ipv4" ]; then
    if command -v ip6tables >/dev/null 2>&1; then
        while ip6tables -t mangle -D PREROUTING -j "$CHAIN_NAME6" 2>/dev/null; do :; done
        ip6tables -t mangle -F "$CHAIN_NAME6" 2>/dev/null || true
        ip6tables -t mangle -X "$CHAIN_NAME6" 2>/dev/null || true
    fi

    if [ -f /proc/net/if_inet6 ]; then
        while ip -6 rule show | grep -qE "$RULE_PATTERN"; do
            ip -6 rule del fwmark "$FWMARK" table "$TABLE_ID" 2>/dev/null || break
        done

        if ! ip -6 rule show | grep -qE "lookup +${TABLE_ID}\b"; then
            ip -6 route flush table "$TABLE_ID" 2>/dev/null || true
        fi
    fi
fi

echo "[rog-tproxy] TPROXY cleanup completed."
