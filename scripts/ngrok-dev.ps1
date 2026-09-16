# Launch Lexsus with the current ngrok tunnel host whitelisted for the MCP
# connector. Free ngrok hands out a new random subdomain every restart, and the
# connector's DNS-rebinding guard only accepts loopback unless the tunnel host
# is listed in LEXSUS_MCP_ALLOWED_HOSTS. This reads the live tunnel from ngrok's
# local API so the allowlist always matches what the providers are dialing.
#
# Usage:
#   1. In one terminal:  ngrok http 45147
#   2. In another:       .\scripts\ngrok-dev.ps1
[CmdletBinding()]
param(
    [string]$NgrokApi = "http://127.0.0.1:4040/api/tunnels"
)

$ErrorActionPreference = "Stop"

function Get-NgrokHost {
    try {
        $tunnels = (Invoke-RestMethod -Uri $NgrokApi -TimeoutSec 5).tunnels
    } catch {
        throw "ngrok API not reachable at $NgrokApi - start 'ngrok http 45147' first."
    }
    $https = $tunnels | Where-Object { $_.proto -eq 'https' } | Select-Object -First 1
    if (-not $https) { $https = $tunnels | Select-Object -First 1 }
    if (-not $https) { throw "No ngrok tunnel found - start 'ngrok http 45147' first." }
    return ([Uri]$https.public_url).Host
}

$ngrokHost = Get-NgrokHost
$env:LEXSUS_MCP_ALLOWED_HOSTS = $ngrokHost

Write-Host "[ngrok-dev] tunnel host:   $ngrokHost"
Write-Host "[ngrok-dev] connector URL: https://$ngrokHost/mcp"

Get-Process ai-continuity-bridge -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "[ngrok-dev] stopping stale app PID $($_.Id)"
    Stop-Process -Id $_.Id -Force
}

Write-Host "[ngrok-dev] launching: pnpm tauri dev"
pnpm tauri dev
