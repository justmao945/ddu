# Signing, code identity and TCC grants

`ddu` is signed with a local self-signed certificate (`Day Day Up Local
Signing`) instead of ad-hoc. This is the long form of the short rule in
AGENTS.md.

## Why the identity has to be stable

macOS stores an app's permissions against its *designated requirement*
(`SecRequirement`). Ad-hoc signing's requirement is a bare `cdhash` — a hash
of the code itself — so every rebuild produced a process whose requirement no
longer matched the stored grant:

```
$ log show --last 10m --predicate 'subsystem == "com.apple.TCC"' | grep -i screen
tccd  Failed to match existing code requirement for subject dev.just.ddu
      and service kTCCServiceScreenCapture
```

The 系统设置 → 隐私与安全性 → 屏幕录制 toggle stayed on while every request
failed (`screencapture`: "could not create image from display"; the agent
harness: `PermissionDenied: macOS Screen Recording permission is not granted`).
The same identity-keyed mechanism decides whether the app may post
notifications, which is why an unsigned build calls
`show_system_notification` and macOS silently drops it (`UNErrorDomain
Code=1`, usernotificationsd "not allowed").

With a certificate the requirement is a function of the certificate, not of
the code:

```
$ codesign -d -r- /Applications/ddu.app/Contents/MacOS/ddu.bin
designated => identifier "dev.just.ddu" and certificate root = H"c6c91d9c79eb3c83e533b03bd7fb286418472d90"
```

`certificate root` is the certificate's SHA-1, `identifier` is the bundle id
`make-bundle.sh` passes with `-i`. Both are inputs, so the requirement survives
any number of rebuilds. (Signing two unrelated binaries with the same identity
prints the same requirement and different cdhashes — that is the whole point.)

## Whose permission it is

Not the agent CLI's. `ddu` spawns `omp` in a PTY, so TCC attributes the
request to the *responsible process* — the app, `dev.just.ddu`, which is the
subject named in the log line above. Granting Terminal, iTerm or `omp` does
nothing.

## Where the pieces live

| piece | location |
|---|---|
| identity | `~/Library/Keychains/ddu-signing.keychain-db` — **the keychain file is the identity**, so it is the thing to back up or copy |
| keychain password | login-keychain item (`security find-generic-password -a ddu -s ddu-signing.keychain -w`); `DDU_SIGN_KEYCHAIN_PW` overrides |
| trust setting | the user trust settings (login keychain) |
| search-list membership | the user keychain search list |

Nothing else is written anywhere: no private-key copy on disk, and no file in
ddu's config directory — that directory is for ddu's configuration, and a
signing identity is not configuration. Provisioning is
`scripts/make-signing-identity.sh`.

## Provisioning and reproducing

```
scripts/make-signing-identity.sh --dry-run   # print the steps
scripts/make-signing-identity.sh             # do it
```

It is idempotent: with an identity present it unlocks the keychain, re-asserts
the trust setting and the search-list membership, then signs a throwaway
binary and prints the requirement. It never generates a second certificate by
itself — a new certificate is a new requirement, so the grants would have to be
re-added by hand. That stays deliberate: `rm
~/Library/Keychains/ddu-signing.keychain-db` and rerun.

To reproduce the identity on another machine, copy that keychain file and run

```
DDU_SIGN_KEYCHAIN_PW=<the keychain password> scripts/make-signing-identity.sh
```

which stores the password in that machine's login keychain. The same
certificate means the same requirement, so grants approved on the old machine
keep matching.

## Why the setup looks fussy

* `openssl pkcs12 -export` needs `-legacy`. Security cannot verify OpenSSL 3's
  default PBES2 MAC: `SecKeychainItemImport: MAC verification failed during
  PKCS12 import (wrong password?)`.
* The key must be in its **own** keychain, not the login keychain. A key
  imported into the login keychain raises an authorization dialog on every
  `codesign` (even with `-A`), because suppressing it means setting the key's
  partition list, and `security set-key-partition-list` on the login keychain
  requires the login password.
* The certificate needs **explicit trust**. `security find-identity -v` reports
  an untrusted self-signed cert as `CSSMERR_TP_NOT_TRUSTED`; an untrusted root
  can never satisfy the stored requirement, and `codesign` refuses to sign with
  an identity it does not consider valid.
* The keychain must be in the user keychain **search list**. `codesign`
  resolves identities through it and `--keychain <path>` alone answers
  `no identity found`.

## Checking

```
codesign -d -r- /Applications/ddu.app/Contents/MacOS/ddu.bin   # the requirement
codesign --verify --verbose /Applications/ddu.app              # "satisfies its Designated Requirement"
security find-identity -v -p codesigning                       # valid identities
log show --last 10m --predicate 'subsystem == "com.apple.TCC"' # who asked, why it failed
```

A requirement containing `cdhash` instead of `certificate root` means ad-hoc
signing crept back in, and rebuilds will start invalidating grants again.

## Overrides

`DDU_SIGN_IDENTITY` replaces the local identity entirely (e.g. a real Developer
ID, with its keychain unlocked by the caller). A machine with no local identity
still builds: `make-bundle.sh` falls back to ad-hoc with a warning. A machine
that *has* the keychain but cannot reach the identity fails the build instead,
because signing ad-hoc there would drop the grants.
