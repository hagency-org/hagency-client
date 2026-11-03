Hagency Client 0.6.4 fixes the first-load Agent list, keeps the private-chat service banner up to date, and fixes remote model saves when the web search setting is omitted.

- **My Agents** shows a loading state while the command journal and roster are fetched.
- The private-chat banner keeps checking the Agent assigned to the current conversation.
- Remote model settings use the server's web search default when the field is omitted and preserve an explicit opt-out.

The release is based on Client `f2f4a612b`. In a local desktop run with Server `1cc3024`, the client connected to an existing Codex account using macOS Keychain. A new Agent initialized its encryption identity on the first attempt, then processed one fixed-text task. The client received and decrypted the reply, and both Matrix events were confirmed as Megolm ciphertext on the server.

Download the installer for your platform and check it against `SHA256SUMS.txt`. This is an unsigned, manual-install release for macOS arm64, Windows amd64, and Linux amd64/arm64. Automatic updates are unavailable. The separate real-host Codex sandbox and two-Agent release qualifications remain outstanding.
