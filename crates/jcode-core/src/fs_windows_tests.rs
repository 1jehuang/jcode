use super::*;

// Assert actual NTFS ACL semantics, not the spelling/count of inherited ACEs.
fn powershell(root: &Path, script: &str) {
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!(r#"
$ErrorActionPreference='Stop'
function Assert-PrivateAcl($Acl, [string]$ExpectedSid, [string]$Context) {{
    $raw=[Security.AccessControl.RawSecurityDescriptor]::new($Acl.GetSecurityDescriptorBinaryForm(),0)
    if($null -eq $raw.DiscretionaryAcl -or $raw.DiscretionaryAcl.Count -eq 0) {{throw "$Context has null or empty DACL"}}
    $rules=@($Acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
    if($rules.Count -eq 0) {{throw "$Context has no access rules"}}
    [int64]$effective=0
    foreach($rule in $rules) {{
        if($rule.IdentityReference.Value -ne $ExpectedSid) {{throw "$Context grants another SID"}}
        if($rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow) {{throw "$Context has non-Allow ACE"}}
        if(($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -eq 0) {{
            $effective=$effective -bor [int64]$rule.FileSystemRights
        }}
    }}
    [int64]$full=[Security.AccessControl.FileSystemRights]::FullControl
    if(($effective -band $full) -ne $full -and ($effective -band 0x10000000) -eq 0) {{throw "$Context lacks effective FullControl"}}
}}
{script}
"#)])
        .env("JCODE_ACL_TEST_ROOT", root)
        // CI invokes cargo from pwsh (PowerShell 7). Its module path can make
        // Windows PowerShell 5 load incompatible Get-Acl assemblies. Let the
        // child initialize its own native module path instead.
        .env_remove("PSModulePath")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run Windows ACL assertions");
    assert!(
        output.status.success(),
        "ACL assertion failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn directory_hardening_is_target_only_and_new_secrets_stay_private() {
    let root = std::env::temp_dir().join(format!(
        "jcode-acl-target-only-{}-{}",
        std::process::id(),
        rand::random::<u64>(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    powershell(
        &root,
        r#"
$r=$env:JCODE_ACL_TEST_ROOT
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=Get-Acl -LiteralPath $r
$a.SetSecurityDescriptorSddlForm("D:P(A;OICI;FA;;;$sid)(A;OICI;FR;;;WD)")
Set-Acl -LiteralPath $r -AclObject $a
[IO.File]::WriteAllText((Join-Path $r 'existing.txt'),'not a secret')
if(-not (Get-Acl (Join-Path $r 'existing.txt')).Sddl.Contains(';;;WD)')) {throw 'fixture did not inherit Everyone read'}
"#,
    );
    set_directory_permissions_owner_only(&root).unwrap();
    powershell(
        &root,
        r#"
$r=$env:JCODE_ACL_TEST_ROOT
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=Get-Acl -LiteralPath $r
if(-not $a.AreAccessRulesProtected) {throw 'directory DACL not protected'}
Assert-PrivateAcl $a $sid 'directory'
if(-not (Get-Acl (Join-Path $r 'existing.txt')).Sddl.Contains(';;;WD)')) {throw 'hardening changed unrelated existing child access'}
[IO.File]::WriteAllText((Join-Path $r 'future.txt'),'new secret fixture')
Assert-PrivateAcl (Get-Acl (Join-Path $r 'future.txt')) $sid 'future child'
"#,
    );
    // Legacy secret repair stays explicit, rather than rewriting unrelated files.
    set_permissions_owner_only(&root.join("existing.txt")).unwrap();
    powershell(
        &root,
        r#"
$r=$env:JCODE_ACL_TEST_ROOT
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=Get-Acl (Join-Path $r 'existing.txt')
if(-not $a.AreAccessRulesProtected) {throw 'secret file DACL not protected'}
Assert-PrivateAcl $a $sid 'secret file'
"#,
    );
    assert!(set_permissions_owner_only(&root.join("missing.txt")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
