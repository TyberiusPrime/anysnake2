echo "enter github username"
set USERNAME (read)
echo "enter access token: ghp..."
set TOKEN (read)
set -x NIX_CONFIG "access-tokens = github.com="$TOKEN
set -x ANYSNAKE2_GITHUB_API_USERNAME "$USERNAME"
set -x ANYSNAKE2_GITHUB_API_PASSWORD "$TOKEN"
