# Inno Setup calls this once for each executable it needs to sign.
param([Parameter(Mandatory)][string]$Path)

$ErrorActionPreference = 'Stop'
# The preceding application-signing action installs the TrustedSigning module.
Invoke-TrustedSigning -Endpoint $env:AZURE_ENDPOINT `
    -CodeSigningAccountName $env:SIGNING_ACCOUNT `
    -CertificateProfileName $env:CERTIFICATE_PROFILE `
    -Files $Path -FileDigest SHA256 `
    -TimestampRfc3161 'http://timestamp.acs.microsoft.com' -TimestampDigest SHA256
