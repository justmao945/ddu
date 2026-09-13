#!/bin/sh
# Provision the local code-signing identity that make-bundle.sh signs with.
#
# Why this exists (the long version is in docs/SIGNING.md): the app's TCC grants
# (Screen Recording, notifications) are stored against its *designated
# requirement*. Ad-hoc signing's requirement is a bare `cdhash`, so every
# re-sign changed it and each install silently invalidated the grants. A
# self-signed certificate makes the requirement
# `identifier "dev.just.ddu" and certificate root = H"c6c9…"`, stable for
# the life of the certificate — so the certificate is generated once per
# machine and then never regenerated.
#
# Where things live: the keychain sits with the user's other keychains, and
# its password is a personal secret, so it goes in the *login keychain* — not
# in a file next to the keychain, and not in ddu's config directory. No copy
# of the private key is written anywhere: the keychain file is the identity
# (copy it, plus the one password, to reproduce the identity elsewhere).
#
# Usage: scripts/make-signing-identity.sh [--dry-run]
#   --dry-run   print what would happen, touch nothing
#
# Environment:
#   DDU_SIGN_KEYCHAIN_PW   the keychain's password. Needed once on a machine
#                          that received the keychain file from another one;
#                          it is then stored in the login keychain and every
#                          later run and build reads it from there.
#
# Re-running is safe: with the identity already provisioned it only
# re-asserts what a reboot or a lost keychain can undo (unlock, trust
# settings, the keychain search list) and exits. It never generates a new
# certificate by itself — a new certificate changes the requirement, so the
# grants must be re-added by hand; that stays a deliberate `rm` of the
# keychain plus a rerun.
set -e
cd "$(dirname "$0")/.."

SIGN_ID="Day Day Up Local Signing"
KC="$HOME/Library/Keychains/ddu-signing.keychain-db"
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"
PW_SERVICE="ddu-signing.keychain"
PW_ACCOUNT="ddu"
DRY=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY=1 ;;
    *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = Darwin ] || { echo "make-signing-identity: macOS only" >&2; exit 1; }
umask 077

# The password, from the environment (first run on a machine that got the
# keychain from elsewhere) or from the login keychain.
pw() {
  if [ -n "${DDU_SIGN_KEYCHAIN_PW:-}" ]; then
    printf '%s' "$DDU_SIGN_KEYCHAIN_PW"
  else
    security find-generic-password -a "$PW_ACCOUNT" -s "$PW_SERVICE" -w 2>/dev/null || true
  fi
}
# `-T /usr/bin/security`: only the `security` tool reads it, so no dialog
# ever appears for the scripts (`-U` because a re-store must update).
store_pw() {
  security add-generic-password -U -a "$PW_ACCOUNT" -s "$PW_SERVICE" \
    -w "$1" -T /usr/bin/security "$LOGIN_KC" >/dev/null
}
visible() {
  security find-identity -v -p codesigning 2>/dev/null | grep -q "$SIGN_ID"
}

if [ -n "$DRY" ]; then
  if [ -f "$KC" ]; then
    echo "make-signing-identity: $KC exists — would unlock it, re-assert trust"
    echo "  and its keychain-search-list membership, then verify '$SIGN_ID'"
  else
    echo "make-signing-identity: would generate '$SIGN_ID', then create $KC,"
    echo "  import it, trust it and add it to the keychain search list:"
    echo "    openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \\"
    echo "      -subj /CN=$SIGN_ID/O=ddu/C=CN -addext extendedKeyUsage=critical,codeSigning …"
    echo "    openssl pkcs12 -export -legacy …   (then import into $KC)"
  fi
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

PW="$(pw)"
if [ ! -f "$KC" ]; then
  echo "make-signing-identity: provisioning '$SIGN_ID'"
  if [ -z "$PW" ]; then
    PW="$(openssl rand -hex 16)"
    store_pw "$PW"
  else
    echo "make-signing-identity: using DDU_SIGN_KEYCHAIN_PW and remembering it in the login keychain"
    store_pw "$PW"
  fi
  # OpenSSL 3 defaults to PBES2, whose MAC macOS Security cannot verify
  # ("MAC verification failed during PKCS12 import") — hence -legacy. The
  # intermediate key/p12 files live only in this 0700 temp dir.
  openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -keyout "$tmp/key.pem" -out "$tmp/cert.pem" \
    -subj "/CN=$SIGN_ID/O=ddu/C=CN" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning"
  openssl pkcs12 -export -legacy -out "$tmp/identity.p12" \
    -inkey "$tmp/key.pem" -in "$tmp/cert.pem" -passout "pass:$PW"
  security create-keychain -p "$PW" "$KC"
  security set-keychain-settings -lut 21600 "$KC"
  security unlock-keychain -p "$PW" "$KC"
  security import "$tmp/identity.p12" -k "$KC" -P "$PW" -T /usr/bin/codesign -A
  # The keychain's own password and partition list are what keep a build
  # from raising an authorization dialog on every codesign. Importing into
  # the *login* keychain instead is not an option: its partition list can
  # only be set with the login password, and without a partition list
  # securityd asks the user on each and every use.
  security set-key-partition-list -S apple-tool:,apple: -s -k "$PW" "$KC" >/dev/null
else
  echo "make-signing-identity: '$SIGN_ID' keychain exists, re-asserting its setup"
  [ -n "$PW" ] || {
    echo "make-signing-identity: no keychain password — set DDU_SIGN_KEYCHAIN_PW (once;" >&2
    echo "  it is then stored in the login keychain and builds read it from there)" >&2
    exit 1
  }
  [ -n "$(pw)" ] || store_pw "$PW"
  # It locks on sleep, so a build unlocks it with the stored password.
  security unlock-keychain -p "$PW" "$KC"
fi

# Trust: without it `security find-identity -v` reports the self-signed cert
# as CSSMERR_TP_NOT_TRUSTED, and an untrusted root can never satisfy the
# requirement TCC stores (codesign also refuses to sign with an identity it
# does not consider valid). The cert is exported on the fly — nothing is
# kept on disk.
if ! visible; then
  echo "make-signing-identity: adding trust settings (macOS may ask for authorization)"
  security find-certificate -c "$SIGN_ID" -p "$KC" > "$tmp/cert.pem"
  security add-trusted-cert -r trustRoot -p codeSign -k "$LOGIN_KC" "$tmp/cert.pem"
fi

# codesign resolves identities through the keychain *search list*; with
# --keychain alone it answers "no identity found". Append, preserving the
# existing entries — the read loop keeps paths containing spaces intact.
if ! security list-keychains -d user | grep -qF "\"$KC\""; then
  echo "make-signing-identity: adding $KC to the keychain search list"
  set --
  while IFS= read -r line; do
    set -- "$@" "$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/^"//; s/"$//')"
  done <<EOF
$(security list-keychains -d user)
EOF
  security list-keychains -d user -s "$@" "$KC"
fi

visible || { echo "make-signing-identity: '$SIGN_ID' still not visible to codesign" >&2; exit 1; }

# End-to-end check: sign a throwaway binary and read back the requirement
# that TCC will store. It must be the certificate-based one; a bare cdhash
# here would mean ad-hoc signing crept back in.
cp /usr/bin/true "$tmp/check.bin"
codesign --force --sign "$SIGN_ID" -i dev.just.ddu "$tmp/check.bin"
req="$(codesign -d -r- "$tmp/check.bin" 2>&1 | tail -1)"
case "$req" in
  *'certificate root = H"'*) ;;
  *) echo "make-signing-identity: requirement is not certificate-based: $req" >&2; exit 1 ;;
esac
echo "provisioned: $req"
