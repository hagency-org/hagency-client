Hagency Client 0.6.5 fixes Agent attachment reception, private-chat runtime status, and the sidebar approval inbox.

- Agents can receive encrypted attachments served through delegated object-storage redirects, then read and return the original file after the required approvals. Existing size, integrity, transport and credential-isolation checks remain in place.
- The private-chat service banner refreshes correctly after switching between Projects and Contacts, and follows the Agent assigned to the exact conversation.
- The left-sidebar approval dialog handles pointer input before the covered workspace. Review and Close respond correctly, and Review selects the original Agent, conversation and request.

The fixes are included in PR #49 and PR #51. Native macOS testing against `hagency.meldry.com` verified attachment receive/read/send/download with matching original bytes, and sidebar approval discovery, exact navigation, approval, rejection and the full 300-second expiry. Rejected and expired requests did not call the model. The App library passed 886 tests with two existing ignored tests; full release build results are linked separately.

Download the installer for your platform and check it against `SHA256SUMS.txt`. This is an unsigned, manual-install release for macOS arm64, Windows amd64, and Linux amd64/arm64. Automatic updates are unavailable. The separate real-host Codex sandbox and two-Agent release qualifications remain outstanding.
