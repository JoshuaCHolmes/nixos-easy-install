# Security Considerations

## Threat Model

NixOS Easy Install runs with administrator privileges on Windows and performs
sensitive operations including:

- Creating files on the EFI System Partition
- Modifying UEFI boot entries
- Downloading executables from the internet
- Handling user passwords

## Mitigations

### Download Security

1. **HTTPS Only**: All downloads use HTTPS to prevent MITM attacks
2. **Checksum Verification**: SHA256 checksums verify boot asset integrity
3. **Trusted Sources**: Assets come from Ubuntu's official archive
4. **Signed Boot Chain**: Using Microsoft/Canonical signed binaries

### Password Handling

1. **SHA-512 Crypt**: Passwords hashed using industry-standard algorithm
2. **Random Salt**: Each password hash uses a unique 16-character salt
3. **5000 Rounds**: Default rounds for SHA-512 crypt
4. **No Plaintext Storage**: Password immediately hashed, never stored raw
5. **Config Cleanup**: install-config.json removed after installation

### Boot Security

1. **Secure Boot Compatible**: Uses pre-signed shim from Ubuntu
2. **No MOK Enrollment**: No custom keys added to machine owner key database
3. **Microsoft-Signed Shim**: First-stage loader signed by Microsoft
4. **Canonical-Signed GRUB**: Second-stage loader signed by Canonical
5. **Windows Untouched**: Never modifies Windows boot manager

### File System Safety

1. **Additive Only**: Only creates new files, never modifies existing
2. **Isolated Directory**: All NixOS files in single folder (C:\NixOS)
3. **Easy Removal**: Complete uninstall by deleting folder + boot entry
4. **No Partition Changes**: Loopback install doesn't touch partitions
5. **Preflight Checks**: Validates everything before making changes

### Privilege Model

1. **Admin Required**: UAC prompt clearly indicates elevation needed
2. **Minimal Scope**: Only elevated operations that require it
3. **No Persistent Services**: Installer doesn't install background services
4. **Transparent Actions**: All operations logged

## Known Limitations

### Loopback Performance

- Slight performance overhead vs native partition
- Not suitable for production servers
- Adequate for desktop/laptop use

### NTFS Dependency

- NixOS root filesystem stored on NTFS via ntfs-3g
- Relies on Windows filesystem stability
- Disk corruption could affect both OSes

### Shared Storage

- If loopback file on same disk as Windows
- SSD TRIM may not work optimally
- No isolation from Windows disk failures

## Recommendations

1. **Keep Backups**: Always maintain backups of important data
2. **Use Dedicated Hardware**: For critical workloads, use separate machine
3. **Monitor Disk Health**: Both OSes on same physical disk
4. **Update Regularly**: Keep both Windows and NixOS updated
5. **Secure Boot**: Leave enabled for maximum security

## Incident Response

If you suspect compromise:

1. **Boot to Windows**: Verify Windows still boots normally
2. **Check Boot Entries**: `bcdedit /enum all` - remove unexpected entries
3. **Scan for Malware**: Use Windows Defender or equivalent
4. **Remove NixOS**: Delete C:\NixOS and EFI\NixOS folders
5. **Reset Secure Boot**: Clear MOK database if any keys were enrolled

## Reporting Security Issues

If you find a security vulnerability:

1. **Do not open a public issue**
2. Email: [TBD - security contact]
3. Include: Description, reproduction steps, potential impact
4. Allow reasonable time for fix before disclosure
