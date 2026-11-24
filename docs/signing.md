# Signing the release files

The Release builds workflow (`.github/workflows/release.yml`) signs each
file whose secrets are set, and builds it unsigned with a warning in the run
where they are not. Nothing here is needed for a pull request's checks.

| File | How it is signed | Needs |
| --- | --- | --- |
| Every file | GitHub attestation (Sigstore), checked with `gh attestation verify <file> --repo armandleopold/leto` | Nothing |
| Android APK | Leto's release key, the same for every release | `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEY_PASSWORD`, variable `ANDROID_CERT_SHA256` |
| Windows `leto.exe`, `leto-cli.exe` | Authenticode, certificate of the SignPath Foundation | `SIGNPATH_API_TOKEN`, variable `SIGNPATH_ORGANIZATION_ID` |
| macOS `leto`, `leto-cli` | Developer ID, hardened runtime, notarized by Apple | `APPLE_*` secrets below |
| Linux | Attestation only | Nothing |

Secrets and variables go in the repository's Settings, "Secrets and
variables", "Actions": secrets on the "Secrets" tab, variables on the
"Variables" tab.

## Android

A phone only installs an update signed with the key of the version it has.
The release key never changes: lose it, and no phone can update Leto again
without uninstalling it.

- `ANDROID_KEYSTORE_BASE64`: the PKCS#12 keystore, in base64
  (`base64 -w0 leto-release.keystore`). The key's alias is `leto`.
- `ANDROID_KEY_PASSWORD`: its password, which is also the key's.
- Variable `ANDROID_CERT_SHA256`: the SHA-256 of the key's certificate, as
  `keytool -list -v` prints it. A release whose APK is signed with another
  key then fails instead of reaching phones.

`cargo xtask android` signs with the same key when `LETO_ANDROID_KEYSTORE`
names the keystore and `LETO_ANDROID_KEY_PASSWORD` holds its password.

## Windows, through SignPath

The [SignPath Foundation](https://signpath.org) signs open source projects
for free, with its own certificate. Its conditions
([terms](https://signpath.org/terms)) are met by Leto as long as:

- every component stays under an OSI licence, with nothing proprietary;
- the signed files are only ever built by this workflow on GitHub's runners;
- the site keeps its "Code signing policy" section (`site/index.html`,
  `#code-signing`), naming who commits, reviews and approves;
- everyone in the team uses two-factor authentication on GitHub and SignPath.

Once SignPath has accepted the project:

1. In SignPath, add the predefined trusted build system "GitHub.com" to the
   organization and link it to the project. Installing the SignPath GitHub
   App on the repository is optional.
2. Make the project with the slug `leto` (or set the variable
   `SIGNPATH_PROJECT_SLUG`), and a signing policy `release-signing` (or set
   `SIGNPATH_SIGNING_POLICY_SLUG`) with the Foundation's certificate.
3. Give the project this artifact configuration. The workflow hands
   SignPath a GitHub artifact, which is a zip of the two programs:

   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file path="leto.exe" product-name="Leto">
         <authenticode-sign/>
       </pe-file>
       <pe-file path="leto-cli.exe" product-name="Leto">
         <authenticode-sign/>
       </pe-file>
     </zip-file>
   </artifact-configuration>
   ```

4. Make an API token for a user who may submit to that policy, and set it as
   the secret `SIGNPATH_API_TOKEN`. Set the organization's ID as the
   variable `SIGNPATH_ORGANIZATION_ID`.

From then on, the Windows job of a release sends the programs to SignPath
and waits, up to a day, for a maintainer to approve the request on
signpath.io. The signed programs go in the zip; a program that comes back
unsigned fails the job.

A signed program still starts with a SmartScreen warning until enough
people have run it: the reputation belongs to the certificate and builds up
over time.

## macOS

Needs a paid Apple Developer Program membership.

- `APPLE_CERTIFICATE_P12_BASE64`: a "Developer ID Application" certificate
  with its private key, exported from Keychain Access as .p12, in base64.
- `APPLE_CERTIFICATE_PASSWORD`: the .p12's password.
- `APPLE_API_KEY_P8_BASE64`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`: an
  App Store Connect API key ("Users and Access", "Integrations"), for the
  notary service.

The programs are signed with the hardened runtime and the entitlements in
`.github/release/macos.entitlements` (camera and microphone), then sent to
Apple's notary service. Bare programs cannot carry the notarization ticket,
so Gatekeeper asks Apple about them on their first start.
