#!/usr/bin/env bash
# ==============================================================================
# rog Universal TPROXY + UDP DNS Setup Script (IPv4 / IPv6 Dual-Stack)
# Usage: ./tproxy-up.sh [TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]
#
# Positional Arguments (all optional):
#   TPROXY_PORT : TPROXY listening port (default: 12345, or $ROG_PORT)
#   FWMARK      : Policy routing fwmark (default: 0x77, or $ROG_FWMARK)
#   TABLE_ID    : Policy routing table ID (default: 177, or $ROG_TABLE)
#   MODE        : Intercept protocols: "tcp+udp" (default), "tcp", or "udp" ($ROG_MODE)
#   FAMILY      : Address family: "dual" (default), "v4", or "v6" ($ROG_FAMILY)
#
# Environment Variables (auto-detected when omitted):
#   ROG_IFACE       : LAN network interface (auto-detected from default route)
#   ROG_IPV4        : Local IPv4 address on ROG_IFACE (auto-detected)
#   ROG_LAN_CIDR    : Local IPv4 subnet CIDR on ROG_IFACE (auto-detected)
#   ROG_DNS_PORT    : Local UDP DNS port to redirect port 53 to (default: 53, 0 to disable)
#   ROG_USER        : Service user to bypass in OUTPUT chain (default: rog)
#   ROG_GROUP       : Service group to bypass in OUTPUT chain (default: rog)
#   ROG_BYPASS_IPV4 : Space-separated IPv4 CIDRs to bypass TPROXY (optional)
#   ROG_SYSCTL_CONF : Optional sysctl file to load if present
# ==============================================================================

set -euo pipefail

TPROXY_PORT="${1:-${ROG_PORT:-12345}}"
FWMARK="${2:-${ROG_FWMARK:-0x77}}"
TABLE_ID="${3:-${ROG_TABLE:-177}}"
MODE="$(echo "${4:-${ROG_MODE:-tcp+udp}}" | tr '[:upper:]' '[:lower:]')"
FAMILY="$(echo "${5:-${ROG_FAMILY:-dual}}" | tr '[:upper:]' '[:lower:]')"

LAN_IF="${ROG_IFACE:-$(ip -4 route show default | awk '{print $5; exit}')}"
SELF_IPV4="${ROG_IPV4:-$(ip -4 -o addr show dev "$LAN_IF" scope global | awk '{split($4,a,"/"); print a[1]; exit}')}"
LAN_CIDR="${ROG_LAN_CIDR:-$(ip -4 route show dev "$LAN_IF" proto kernel scope link | awk '{print $1; exit}')}"
DNS_PORT="${ROG_DNS_PORT:-53}"
ROG_USER="${ROG_USER:-rog}"
ROG_GROUP="${ROG_GROUP:-rog}"
BYPASS_IPV4="${ROG_BYPASS_IPV4:-}"
SYSCTL_CONF="${ROG_SYSCTL_CONF:-}"

DO_TCP=false
DO_UDP=false
[[ "$MODE" == *"tcp"* ]] && DO_TCP=true
[[ "$MODE" == *"udp"* ]] && DO_UDP=true
if [[ "$DO_TCP" == false && "$DO_UDP" == false ]]; then
    DO_TCP=true
    DO_UDP=true
fi

DO_V4=false
DO_V6=false
case "$FAMILY" in
    v4|ipv4) DO_V4=true ;;
    v6|ipv6) DO_V6=true ;;
    dual|all|auto|*)
        DO_V4=true
        [[ -f /proc/net/if_inet6 ]] && DO_V6=true
        ;;
esac

ensure_rule() {
    local table="$1" chain="$2"
    shift 2
    iptables -w -t "$table" -C "$chain" "$@" 2>/dev/null || iptables -w -t "$table" -A "$chain" "$@"
}

ensure_rule6() {
    local table="$1" chain="$2"
    shift 2
    ip6tables -w -t "$table" -C "$chain" "$@" 2>/dev/null || ip6tables -w -t "$table" -A "$chain" "$@"
}

modprobe nf_tproxy_ipv4 nf_tproxy_ipv6 xt_TPROXY xt_socket xt_owner 2>/dev/null || true

if [[ -n "$SYSCTL_CONF" && -f "$SYSCTL_CONF" ]]; then
    sysctl --load="$SYSCTL_CONF" >/dev/null
else
    sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.all.send_redirects=0 >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.default.send_redirects=0 >/dev/null 2>&1 || true
    sysctl -w "net.ipv4.conf.${LAN_IF}.send_redirects=0" >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.all.accept_redirects=0 >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.default.accept_redirects=0 >/dev/null 2>&1 || true
    sysctl -w "net.ipv4.conf.${LAN_IF}.accept_redirects=0" >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.all.rp_filter=0 >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.default.rp_filter=0 >/dev/null 2>&1 || true
    sysctl -w "net.ipv4.conf.${LAN_IF}.rp_filter=0" >/dev/null 2>&1 || true
    sysctl -w net.ipv4.conf.lo.rp_filter=0 >/dev/null 2>&1 || true
    if [[ "$DO_V6" == true ]]; then
        sysctl -w net.ipv6.conf.all.forwarding=1 >/dev/null 2>&1 || true
        sysctl -w net.ipv6.conf.default.forwarding=1 >/dev/null 2>&1 || true
        sysctl -w "net.ipv6.conf.${LAN_IF}.forwarding=1" >/dev/null 2>&1 || true
        sysctl -w net.ipv6.conf.all.accept_ra=2 >/dev/null 2>&1 || true
        sysctl -w net.ipv6.conf.default.accept_ra=2 >/dev/null 2>&1 || true
        sysctl -w "net.ipv6.conf.${LAN_IF}.accept_ra=2" >/dev/null 2>&1 || true
        sysctl -w net.ipv6.conf.all.accept_redirects=0 >/dev/null 2>&1 || true
        sysctl -w net.ipv6.conf.default.accept_redirects=0 >/dev/null 2>&1 || true
        sysctl -w "net.ipv6.conf.${LAN_IF}.accept_redirects=0" >/dev/null 2>&1 || true
    fi
fi

UID_ROG="$(id -u "$ROG_USER" 2>/dev/null || true)"
GID_ROG="$(id -g "$ROG_GROUP" 2>/dev/null || true)"

# ------------------------------------------------------------------------------
# 1. IPv4 Setup
# ------------------------------------------------------------------------------
if [[ "$DO_V4" == true ]]; then
    ip -4 rule show | grep -q "^${TABLE_ID}:" || ip -4 rule add pref "$TABLE_ID" fwmark "${FWMARK}/0xff" lookup "$TABLE_ID"
    ip -4 route replace local 0.0.0.0/0 dev lo table "$TABLE_ID"

    for chain in ROG_TP ROG_OUT; do
        iptables -w -t mangle -N "$chain" 2>/dev/null || iptables -w -t mangle -F "$chain"
    done
    for chain in ROG_NAT ROG_DNS_PRE ROG_DNS_OUT; do
        iptables -w -t nat -N "$chain" 2>/dev/null || iptables -w -t nat -F "$chain"
    done

    for chain in ROG_TP ROG_OUT; do
        iptables -w -t mangle -A "$chain" -m addrtype --dst-type LOCAL -j RETURN
        [[ "$DNS_PORT" != "0" ]] && iptables -w -t mangle -A "$chain" -p udp --dport 53 -j RETURN
        for network in 0.0.0.0/8 10.0.0.0/8 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.168.0.0/16 198.18.0.0/15 224.0.0.0/4 240.0.0.0/4; do
            iptables -w -t mangle -A "$chain" -d "$network" -j RETURN
        done
        for cidr in $BYPASS_IPV4; do
            iptables -w -t mangle -A "$chain" -d "$cidr" -j RETURN
        done
    done

    iptables -w -t mangle -I ROG_OUT 1 -m addrtype ! --src-type LOCAL -j RETURN
    [[ -n "$UID_ROG" ]] && iptables -w -t mangle -A ROG_OUT -m owner --uid-owner "$UID_ROG" -j RETURN
    [[ -n "$GID_ROG" ]] && iptables -w -t mangle -A ROG_OUT -m owner --gid-owner "$GID_ROG" -j RETURN
    [[ "$DO_TCP" == true ]] && iptables -w -t mangle -A ROG_OUT -p tcp -j MARK --set-xmark "${FWMARK}/0xff"
    [[ "$DO_UDP" == true ]] && iptables -w -t mangle -A ROG_OUT -p udp -j MARK --set-xmark "${FWMARK}/0xff"

    [[ "$DO_TCP" == true ]] && iptables -w -t mangle -A ROG_TP -p tcp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "${FWMARK}/0xff"
    [[ "$DO_UDP" == true ]] && iptables -w -t mangle -A ROG_TP -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "${FWMARK}/0xff"

    ensure_rule mangle PREROUTING -i "$LAN_IF" -s "$LAN_CIDR" -j ROG_TP
    ensure_rule mangle PREROUTING -i lo -m mark --mark "${FWMARK}/0xff" -j ROG_TP
    ensure_rule mangle OUTPUT -j ROG_OUT

    if [[ "$DNS_PORT" != "0" && -n "$SELF_IPV4" ]]; then
        iptables -w -t nat -A ROG_DNS_PRE -p udp --dport 53 ! -d "${SELF_IPV4}/32" -j DNAT --to-destination "${SELF_IPV4}:${DNS_PORT}"
        iptables -w -t nat -A ROG_DNS_OUT -m addrtype ! --src-type LOCAL -j RETURN
        [[ -n "$UID_ROG" ]] && iptables -w -t nat -A ROG_DNS_OUT -m owner --uid-owner "$UID_ROG" -j RETURN
        [[ -n "$GID_ROG" ]] && iptables -w -t nat -A ROG_DNS_OUT -m owner --gid-owner "$GID_ROG" -j RETURN
        iptables -w -t nat -A ROG_DNS_OUT -p udp --dport 53 ! -d "${SELF_IPV4}/32" -j DNAT --to-destination "${SELF_IPV4}:${DNS_PORT}"
        ensure_rule nat PREROUTING -i "$LAN_IF" -s "$LAN_CIDR" -j ROG_DNS_PRE
        ensure_rule nat OUTPUT -j ROG_DNS_OUT
    fi

    iptables -w -t nat -A ROG_NAT -s "$LAN_CIDR" -o "$LAN_IF" -j MASQUERADE
    ensure_rule nat POSTROUTING -j ROG_NAT
fi

# ------------------------------------------------------------------------------
# 2. IPv6 Setup
# ------------------------------------------------------------------------------
if [[ "$DO_V6" == true ]] && command -v ip6tables >/dev/null 2>&1; then
    ip -6 rule show | grep -q "^${TABLE_ID}:" || ip -6 rule add pref "$TABLE_ID" fwmark "${FWMARK}/0xff" lookup "$TABLE_ID"
    ip -6 route replace local ::/0 dev lo table "$TABLE_ID"

    for chain in ROG6_TP ROG6_OUT; do
        ip6tables -w -t mangle -N "$chain" 2>/dev/null || ip6tables -w -t mangle -F "$chain"
    done
    for chain in ROG6_DNS_PRE ROG6_DNS_OUT; do
        ip6tables -w -t nat -N "$chain" 2>/dev/null || ip6tables -w -t nat -F "$chain"
    done

    for chain in ROG6_TP ROG6_OUT; do
        ip6tables -w -t mangle -A "$chain" -m addrtype --dst-type LOCAL -j RETURN
        [[ "$DNS_PORT" != "0" ]] && ip6tables -w -t mangle -A "$chain" -p udp --dport 53 -j RETURN
        for network in ::/128 ::1/128 fc00::/7 fe80::/10 ff00::/8; do
            ip6tables -w -t mangle -A "$chain" -d "$network" -j RETURN
        done
        for prefix in $(ip -6 route show dev "$LAN_IF" | awk '/::\// && !/^default/ && !/^fe80:/ {print $1}' | sort -u); do
            ip6tables -w -t mangle -A "$chain" -d "$prefix" -j RETURN
        done
    done

    ip6tables -w -t mangle -I ROG6_OUT 1 -m addrtype ! --src-type LOCAL -j RETURN
    [[ -n "$UID_ROG" ]] && ip6tables -w -t mangle -A ROG6_OUT -m owner --uid-owner "$UID_ROG" -j RETURN
    [[ -n "$GID_ROG" ]] && ip6tables -w -t mangle -A ROG6_OUT -m owner --gid-owner "$GID_ROG" -j RETURN
    [[ "$DO_TCP" == true ]] && ip6tables -w -t mangle -A ROG6_OUT -p tcp -j MARK --set-xmark "${FWMARK}/0xff"
    [[ "$DO_UDP" == true ]] && ip6tables -w -t mangle -A ROG6_OUT -p udp -j MARK --set-xmark "${FWMARK}/0xff"

    [[ "$DO_TCP" == true ]] && ip6tables -w -t mangle -A ROG6_TP -p tcp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "${FWMARK}/0xff"
    [[ "$DO_UDP" == true ]] && ip6tables -w -t mangle -A ROG6_TP -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "${FWMARK}/0xff"

    ensure_rule6 mangle PREROUTING -i "$LAN_IF" -j ROG6_TP
    ensure_rule6 mangle PREROUTING -i lo -m mark --mark "${FWMARK}/0xff" -j ROG6_TP
    ensure_rule6 mangle OUTPUT -j ROG6_OUT

    if [[ "$DNS_PORT" != "0" ]]; then
        ip6tables -w -t nat -A ROG6_DNS_PRE -p udp --dport 53 -j REDIRECT --to-ports "$DNS_PORT"
        ip6tables -w -t nat -A ROG6_DNS_OUT -m addrtype ! --src-type LOCAL -j RETURN
        [[ -n "$UID_ROG" ]] && ip6tables -w -t nat -A ROG6_DNS_OUT -m owner --uid-owner "$UID_ROG" -j RETURN
        [[ -n "$GID_ROG" ]] && ip6tables -w -t nat -A ROG6_DNS_OUT -m owner --gid-owner "$GID_ROG" -j RETURN
        ip6tables -w -t nat -A ROG6_DNS_OUT -p udp --dport 53 -m addrtype ! --dst-type LOCAL -j REDIRECT --to-ports "$DNS_PORT"
        ensure_rule6 nat PREROUTING -i "$LAN_IF" -j ROG6_DNS_PRE
        ensure_rule6 nat OUTPUT -j ROG6_DNS_OUT
    fi
fi
