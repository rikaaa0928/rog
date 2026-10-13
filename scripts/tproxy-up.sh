#!/bin/bash
# ==============================================================================
# rog TPROXY Setup Script
# Usage: ./tproxy-up.sh [TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]
#
# Arguments:
#   TPROXY_PORT : Port that rog is listening on (default: 12345)
#   FWMARK      : Firewall mark for policy routing (default: 1)
#   TABLE_ID    : Route table number (default: 100)
#   MODE        : Protocols to intercept: "tcp+udp" (default), "tcp", or "udp"
#   FAMILY      : Address family to intercept: "v4" (default), "v6", or "dual"
# ==============================================================================

set -e

TPROXY_PORT=${1:-${ROG_PORT:-12345}}
FWMARK=${2:-${ROG_FWMARK:-1}}
TABLE_ID=${3:-${ROG_TABLE:-100}}
MODE=${4:-${ROG_MODE:-tcp+udp}}
FAMILY=${5:-${ROG_FAMILY:-v4}}

MODE=$(echo "$MODE" | tr '[:upper:]' '[:lower:]')
FAMILY=$(echo "$FAMILY" | tr '[:upper:]' '[:lower:]')

CHAIN_NAME="ROG_TPROXY"
CHAIN_NAME6="ROG_TPROXY6"

FWMARK_HEX=$(printf "0x%x" "$FWMARK" 2>/dev/null || echo "$FWMARK")
RULE_PATTERN="fwmark +(${FWMARK}|${FWMARK_HEX})(/0x[0-9a-fA-F]+)?\b.*lookup +${TABLE_ID}\b"
ROUTE4_PATTERN="local +(default|0\.0\.0\.0/0) +dev +lo\b"
ROUTE6_PATTERN="local +(default|::/0) +dev +lo\b"

DO_TCP=false
DO_UDP=false
if [[ "$MODE" == *"tcp"* ]]; then
    DO_TCP=true
fi
if [[ "$MODE" == *"udp"* ]]; then
    DO_UDP=true
fi
if [ "$DO_TCP" = false ] && [ "$DO_UDP" = false ]; then
    DO_TCP=true
    DO_UDP=true
fi

DO_V4=false
DO_V6=false
case "$FAMILY" in
    v4|ipv4)
        DO_V4=true
        ;;
    v6|ipv6)
        DO_V6=true
        ;;
    dual|all|auto)
        DO_V4=true
        if [ -f /proc/net/if_inet6 ]; then
            DO_V6=true
        fi
        ;;
    *)
        DO_V4=true
        ;;
esac

echo "[rog-tproxy] Applying TPROXY rules (port: $TPROXY_PORT, fwmark: $FWMARK, table: $TABLE_ID, mode: $MODE, family: $FAMILY)..."

# ------------------------------------------------------------------------------
# 1. IPv4 Setup & Validation
# ------------------------------------------------------------------------------
if [ "$DO_V4" = true ]; then
    echo "[rog-tproxy] Configuring IPv4 policy routing and iptables..."

    # Check if policy rule already exists; only add if not present
    if ! ip -4 rule show | grep -qE "$RULE_PATTERN"; then
        echo "[rog-tproxy] Adding IPv4 rule: fwmark $FWMARK -> table $TABLE_ID"
        ip -4 rule add fwmark "$FWMARK" table "$TABLE_ID" pref 100
    fi

    # Check if local route already exists in table; only add if not present
    if ! ip -4 route show table "$TABLE_ID" | grep -qE "$ROUTE4_PATTERN"; then
        echo "[rog-tproxy] Adding IPv4 route: local 0.0.0.0/0 dev lo table $TABLE_ID"
        ip -4 route add local 0.0.0.0/0 dev lo table "$TABLE_ID"
    fi

    # Strictly verify routing configuration
    if ! ip -4 rule show | grep -qE "$RULE_PATTERN"; then
        echo "[rog-tproxy] ERROR: IPv4 rule for fwmark $FWMARK -> table $TABLE_ID failed verification!" >&2
        exit 1
    fi
    if ! ip -4 route show table "$TABLE_ID" | grep -qE "$ROUTE4_PATTERN"; then
        echo "[rog-tproxy] ERROR: IPv4 route in table $TABLE_ID on dev lo failed verification!" >&2
        exit 1
    fi

    # Create or flush dedicated chain
    iptables -t mangle -N "$CHAIN_NAME" 2>/dev/null || iptables -t mangle -F "$CHAIN_NAME"

    # Bypass private/reserved IP ranges
    iptables -t mangle -A "$CHAIN_NAME" -d 0.0.0.0/8 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 10.0.0.0/8 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 127.0.0.0/8 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 169.254.0.0/16 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 172.16.0.0/12 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 192.168.0.0/16 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 224.0.0.0/4 -j RETURN
    iptables -t mangle -A "$CHAIN_NAME" -d 240.0.0.0/4 -j RETURN

    # Redirect only configured protocols to this instance's port
    if [ "$DO_TCP" = true ]; then
        iptables -t mangle -A "$CHAIN_NAME" -p tcp -j TPROXY --tproxy-mark "$FWMARK/$FWMARK" --on-port "$TPROXY_PORT"
    fi
    if [ "$DO_UDP" = true ]; then
        iptables -t mangle -A "$CHAIN_NAME" -p udp -j TPROXY --tproxy-mark "$FWMARK/$FWMARK" --on-port "$TPROXY_PORT"
    fi

    # Attach to PREROUTING chain (remove existing first to avoid duplicate entries)
    while iptables -t mangle -D PREROUTING -j "$CHAIN_NAME" 2>/dev/null; do :; done
    iptables -t mangle -I PREROUTING -j "$CHAIN_NAME"

    if ! iptables -t mangle -C PREROUTING -j "$CHAIN_NAME" 2>/dev/null; then
        echo "[rog-tproxy] ERROR: iptables PREROUTING attachment for $CHAIN_NAME failed!" >&2
        exit 1
    fi
fi

# ------------------------------------------------------------------------------
# 2. IPv6 Setup & Validation
# ------------------------------------------------------------------------------
if [ "$DO_V6" = true ]; then
    if ! command -v ip6tables >/dev/null 2>&1; then
        echo "[rog-tproxy] ERROR: IPv6 requested but ip6tables command not found!" >&2
        exit 1
    fi

    echo "[rog-tproxy] Configuring IPv6 policy routing and ip6tables..."

    if ! ip -6 rule show | grep -qE "$RULE_PATTERN"; then
        echo "[rog-tproxy] Adding IPv6 rule: fwmark $FWMARK -> table $TABLE_ID"
        ip -6 rule add fwmark "$FWMARK" table "$TABLE_ID" pref 100
    fi

    if ! ip -6 route show table "$TABLE_ID" | grep -qE "$ROUTE6_PATTERN"; then
        echo "[rog-tproxy] Adding IPv6 route: local ::/0 dev lo table $TABLE_ID"
        ip -6 route add local ::/0 dev lo table "$TABLE_ID"
    fi

    if ! ip -6 rule show | grep -qE "$RULE_PATTERN"; then
        echo "[rog-tproxy] ERROR: IPv6 rule for fwmark $FWMARK -> table $TABLE_ID failed verification!" >&2
        exit 1
    fi
    if ! ip -6 route show table "$TABLE_ID" | grep -qE "$ROUTE6_PATTERN"; then
        echo "[rog-tproxy] ERROR: IPv6 route in table $TABLE_ID on dev lo failed verification!" >&2
        exit 1
    fi

    ip6tables -t mangle -N "$CHAIN_NAME6" 2>/dev/null || ip6tables -t mangle -F "$CHAIN_NAME6"
    ip6tables -t mangle -A "$CHAIN_NAME6" -d ::1/128 -j RETURN
    ip6tables -t mangle -A "$CHAIN_NAME6" -d fc00::/7 -j RETURN
    ip6tables -t mangle -A "$CHAIN_NAME6" -d fe80::/10 -j RETURN

    if [ "$DO_TCP" = true ]; then
        ip6tables -t mangle -A "$CHAIN_NAME6" -p tcp -j TPROXY --tproxy-mark "$FWMARK/$FWMARK" --on-port "$TPROXY_PORT"
    fi
    if [ "$DO_UDP" = true ]; then
        ip6tables -t mangle -A "$CHAIN_NAME6" -p udp -j TPROXY --tproxy-mark "$FWMARK/$FWMARK" --on-port "$TPROXY_PORT"
    fi

    while ip6tables -t mangle -D PREROUTING -j "$CHAIN_NAME6" 2>/dev/null; do :; done
    ip6tables -t mangle -I PREROUTING -j "$CHAIN_NAME6"

    if ! ip6tables -t mangle -C PREROUTING -j "$CHAIN_NAME6" 2>/dev/null; then
        echo "[rog-tproxy] ERROR: ip6tables PREROUTING attachment for $CHAIN_NAME6 failed!" >&2
        exit 1
    fi
fi

echo "[rog-tproxy] TPROXY routing rules applied and verified successfully."
