# Security

`mncs-system-monitor` is experimental and does not currently inspect live host resources. Future
collectors may read process metadata, cgroups, service state, sockets, or other privileged host
surfaces.

Do not add privilege escalation, credential access, or unrestricted host traversal as an implicit
collector behavior. Every authority should be declared, bounded, and represented in collection
status. Please report security issues privately to the repository owner before public disclosure.
