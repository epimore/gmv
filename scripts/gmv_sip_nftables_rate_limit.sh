#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage:
  scripts/gmv_sip_nftables_rate_limit.sh print
  scripts/gmv_sip_nftables_rate_limit.sh apply
  scripts/gmv_sip_nftables_rate_limit.sh status
  scripts/gmv_sip_nftables_rate_limit.sh flush

Environment:
  SIP_PORT=25600
  TABLE_NAME=gmv_sip_guard
  UDP_RATE=10/second
  UDP_BURST=30
  TCP_SYN_RATE=5/second
  TCP_SYN_BURST=20
  BLACKLIST_TIMEOUT=10m
  WHITELIST="10.1.0.0/16,203.0.113.10"

Notes:
  - print is the default and does not modify firewall rules.
  - apply recreates only table inet ${TABLE_NAME:-gmv_sip_guard}.
  - whitelist is evaluated before rate limiting and blacklist drops.
USAGE
}

ACTION="${1:-print}"
SIP_PORT="${SIP_PORT:-25600}"
TABLE_NAME="${TABLE_NAME:-gmv_sip_guard}"
UDP_RATE="${UDP_RATE:-10/second}"
UDP_BURST="${UDP_BURST:-30}"
TCP_SYN_RATE="${TCP_SYN_RATE:-5/second}"
TCP_SYN_BURST="${TCP_SYN_BURST:-20}"
BLACKLIST_TIMEOUT="${BLACKLIST_TIMEOUT:-10m}"
WHITELIST="${WHITELIST:-}"

if [[ ! "$SIP_PORT" =~ ^[0-9]+$ ]] || (( SIP_PORT < 1 || SIP_PORT > 65535 )); then
    echo "invalid SIP_PORT: $SIP_PORT" >&2
    exit 2
fi

if [[ ! "$UDP_BURST" =~ ^[0-9]+$ ]] || [[ ! "$TCP_SYN_BURST" =~ ^[0-9]+$ ]]; then
    echo "UDP_BURST and TCP_SYN_BURST must be integers" >&2
    exit 2
fi

nft_cmd() {
    if (( EUID == 0 )); then
        nft "$@"
    else
        sudo nft "$@"
    fi
}

whitelist_block() {
    if [[ -z "$WHITELIST" ]]; then
        cat <<'EOF'
        set whitelist {
            type ipv4_addr
            flags interval
        }
EOF
        return
    fi

    local elements
    elements="$(printf '%s' "$WHITELIST" | tr -d '[:space:]')"
    cat <<EOF
        set whitelist {
            type ipv4_addr
            flags interval
            elements = { $elements }
        }
EOF
}

render_rules() {
    cat <<EOF
table inet $TABLE_NAME {
$(whitelist_block)

        set blacklist {
            type ipv4_addr
            timeout $BLACKLIST_TIMEOUT
        }

        chain input {
            type filter hook input priority -100; policy accept;

            ip saddr @whitelist udp dport $SIP_PORT accept
            ip saddr @whitelist tcp dport $SIP_PORT accept

            ip saddr @blacklist udp dport $SIP_PORT drop
            ip saddr @blacklist tcp dport $SIP_PORT drop

            udp dport $SIP_PORT meter sip_udp_rate {
                ip saddr limit rate over $UDP_RATE burst $UDP_BURST packets
            } add @blacklist { ip saddr timeout $BLACKLIST_TIMEOUT } drop

            tcp dport $SIP_PORT tcp flags syn meter sip_tcp_syn_rate {
                ip saddr limit rate over $TCP_SYN_RATE burst $TCP_SYN_BURST packets
            } add @blacklist { ip saddr timeout $BLACKLIST_TIMEOUT } drop
        }
}
EOF
}

case "$ACTION" in
    print)
        render_rules
        ;;
    apply)
        if nft_cmd list table inet "$TABLE_NAME" >/dev/null 2>&1; then
            nft_cmd delete table inet "$TABLE_NAME"
        fi
        render_rules | nft_cmd -f -
        nft_cmd list table inet "$TABLE_NAME"
        ;;
    status)
        nft_cmd list table inet "$TABLE_NAME"
        ;;
    flush)
        if nft_cmd list table inet "$TABLE_NAME" >/dev/null 2>&1; then
            nft_cmd delete table inet "$TABLE_NAME"
        fi
        ;;
    -h|--help|help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
