#!/usr/bin/env bash
# ==============================================================================
# rog Universal TPROXY + UDP DNS Teardown Script (IPv4 / IPv6 Dual-Stack)
# Usage: ./tproxy-down.sh [TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]
# ==============================================================================

set -euo pipefail

TPROXY_PORT="${1:-${ROG_PORT:-12345}}"
FWMARK="${2:-${ROG_FWMARK:-0x77}}"
TABLE_ID="${3:-${ROG_TABLE:-177}}"
FAMILY="$(echo "${5:-${ROG_FAMILY:-dual}}" | tr '[:upper:]' '[:lower:]')"

LAN_IF="${ROG_IFACE:-$(ip -4 route show default | awk '{print $5; exit}')}"
LAN_CIDR="${ROG_LAN_CIDR:-$(ip -4 route show dev "$LAN_IF" proto kernel scope link | awk '{print $1; exit}')}"

if [[ "$FAMILY" != "v6" && "$FAMILY" != "ipv6" ]]; then
    while iptables -w -t mangle -D PREROUTING -i "$LAN_IF" -s "$LAN_CIDR" -j ROG_TP 2>/dev/null; do :; done
    while iptables -w -t mangle -D PREROUTING -i lo -m mark --mark "${FWMARK}/0xff" -j ROG_TP 2>/dev/null; do :; done
    while iptables -w -t mangle -D OUTPUT -j ROG_OUT 2>/dev/null; do :; done
    while iptables -w -t nat -D PREROUTING -i "$LAN_IF" -s "$LAN_CIDR" -j ROG_DNS_PRE 2>/dev/null; do :; done
    while iptables -w -t nat -D OUTPUT -j ROG_DNS_OUT 2>/dev/null; do :; done
    while iptables -w -t nat -D POSTROUTING -j ROG_NAT 2>/dev/null; do :; done
    for chain in ROG_TP ROG_OUT; do
        iptables -w -t mangle -F "$chain" 2>/dev/null || true
        iptables -w -t mangle -X "$chain" 2>/dev/null || true
    done
    for chain in ROG_NAT ROG_DNS_PRE ROG_DNS_OUT; do
        iptables -w -t nat -F "$chain" 2>/dev/null || true
        iptables -w -t nat -X "$chain" 2>/dev/null || true
    done
    while ip -4 rule del pref "$TABLE_ID" fwmark "${FWMARK}/0xff" lookup "$TABLE_ID" 2>/dev/null; do :; done
    ip -4 route flush table "$TABLE_ID" 2>/dev/null || true
fi

if [[ "$FAMILY" != "v4" && "$FAMILY" != "ipv4" ]] && command -v ip6tables >/dev/null 2>&1; then
    while ip6tables -w -t mangle -D PREROUTING -i "$LAN_IF" -j ROG6_TP 2>/dev/null; do :; done
    while ip6tables -w -t mangle -D PREROUTING -i lo -m mark --mark "${FWMARK}/0xff" -j ROG6_TP 2>/dev/null; do :; done
    while ip6tables -w -t mangle -D OUTPUT -j ROG6_OUT 2>/dev/null; do :; done
    while ip6tables -w -t nat -D PREROUTING -i "$LAN_IF" -j ROG6_DNS_PRE 2>/dev/null; do :; done
    while ip6tables -w -t nat -D OUTPUT -j ROG6_DNS_OUT 2>/dev/null; do :; done
    for chain in ROG6_TP ROG6_OUT; do
        ip6tables -w -t mangle -F "$chain" 2>/dev/null || true
        ip6tables -w -t mangle -X "$chain" 2>/dev/null || true
    done
    for chain in ROG6_DNS_PRE ROG6_DNS_OUT; do
        ip6tables -w -t nat -F "$chain" 2>/dev/null || true
        ip6tables -w -t nat -X "$chain" 2>/dev/null || true
    done
    while ip -6 rule del pref "$TABLE_ID" fwmark "${FWMARK}/0xff" lookup "$TABLE_ID" 2>/dev/null; do :; done
    ip -6 route flush table "$TABLE_ID" 2>/dev/null || true
fi
