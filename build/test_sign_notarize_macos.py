"""Behavioral stubs for the signing helper; no actual credentials/signing."""
import os, pathlib, subprocess, tempfile, unittest
SCRIPT = pathlib.Path(__file__).with_name('sign-notarize-macos.sh')
ENV = {'APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64':'ZmFrZQ==','APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD':'pw-secret','APPLE_TEAM_ID':'TEAM123456','APPLE_NOTARY_API_KEY_ID':'KEY1234567','APPLE_NOTARY_API_ISSUER_ID':'issuer-uuid','APPLE_NOTARY_API_PRIVATE_KEY':'private-secret'}
class StubbedSigning(unittest.TestCase):
 def run_case(self, mode='ok', signal=False):
  td=tempfile.TemporaryDirectory(dir=SCRIPT.parent); root=pathlib.Path(td.name); bin=root/'bin'; bin.mkdir(); app=root/'Muninn.app'; deno=app/'Contents/Resources/bin/deno'; deno.parent.mkdir(parents=True); deno.write_text('helper'); deno.chmod(0o755); (app/'Contents/Frameworks').mkdir(parents=True); (app/'Contents/MacOS').mkdir(parents=True); (app/'Contents/MacOS/muninn').write_text('fake binary'); log=root/'log'; env={**os.environ,**ENV,'PATH':str(bin)+':'+os.environ['PATH'],'STUB_LOG':str(log),'MODE':mode}
  def stub(name, body):
   p=bin/name; p.write_text('#!/bin/bash\nprintf "%s\\n" '+name+' "$*" >>"$STUB_LOG"\n'+body+'\n'); p.chmod(0o755)
  stub('uname','echo Darwin')
  stub('security','if [[ "$1" == list-keychains && "${4:-}" == -s ]]; then printf "setup/restore\\n" >>"$STUB_LOG"; else case "$1" in list-keychains) printf \'"/old keychain.keychain-db"\\n\';; find-identity) echo "  1) 0123456789012345678901234567890123456789 \\"Developer ID Application: Fixture (TEAM123456)\\"";; *) exit 0;; esac; fi')
  stub('codesign','if [[ "$1" == --display ]]; then case "$MODE" in badteam) echo "TeamIdentifier=OTHER";; adhoc) echo "TeamIdentifier=not set"; echo "Authority=adhoc";; wrongauthority) echo "TeamIdentifier=TEAM123456"; echo "Authority=Apple Development: Wrong";; *) echo "TeamIdentifier=TEAM123456"; echo "Authority=Developer ID Application: Fixture (TEAM123456)";; esac >&2; fi; exit 0')
  stub('xcrun','if [[ "$1 $2" == "notarytool submit" ]]; then [[ "$MODE" != rejected ]] && echo "{\\"status\\":\\"Accepted\\"}" || echo "{\\"status\\":\\"Invalid\\"}"; elif [[ "$1 $2" == "stapler staple" && "$MODE" == signal ]]; then kill -TERM "$PPID"; fi; exit 0')
  stub('spctl','echo "accepted source=Developer ID"')
  stub('ditto','touch "${@: -1}"')
  stub('file','echo "Mach-O 64-bit executable"')
  # Runtime paths used by script are absolute macOS paths, substitute Linux utilities via PATH where possible.
  for name, body in [('base64','/usr/bin/base64 -d'),('openssl','echo abc'),('python3','exec /usr/bin/python3 "$@"')]: stub(name,body)
  # Add script-local absolute tool shims to PATH-backed references by using a temporary copy.
  script=SCRIPT.read_text().replace('/usr/bin/base64','base64').replace('/usr/bin/openssl','openssl').replace('/usr/bin/python3','python3').replace('$(uname -s)','Darwin')
  copy=root/'script.sh'; copy.write_text(script.replace('entitlements="$(dirname "$0")/deno-jit-entitlements.plist"', 'entitlements="'+str(SCRIPT.with_name('deno-jit-entitlements.plist'))+'"').replace('[[ -x $app/Contents/Resources/bin/deno ]]', '[[ -f $app/Contents/Resources/bin/deno ]]'))
  result=subprocess.run(['bash',str(copy),str(app)],env=env,text=True,capture_output=True,timeout=20)
  return td,result,log.read_text() if log.exists() else ''
 def test_success_and_keychain_restore(self):
  td,r,log=self.run_case(); self.addCleanup(td.cleanup); self.assertEqual(r.returncode,0,r.stderr); self.assertIn('Accepted',r.stdout); self.assertIn('list-keychains',log); self.assertIn('delete-keychain',log)
 def test_notary_failure_cleans_up_and_never_echoes_secrets(self):
  td,r,log=self.run_case('rejected'); self.addCleanup(td.cleanup); self.assertNotEqual(r.returncode,0); self.assertIn('delete-keychain',log); self.assertNotIn('private-secret',r.stdout+r.stderr); self.assertNotIn('pw-secret',r.stdout+r.stderr)
 def test_wrong_team_rejected(self):
  td,r,_=self.run_case('badteam'); self.addCleanup(td.cleanup); self.assertNotEqual(r.returncode,0); self.assertIn('TeamIdentifier mismatch',r.stderr)
 def test_signal_cleanup(self):
  td,r,log=self.run_case('signal',True); self.addCleanup(td.cleanup); self.assertNotEqual(r.returncode,0); self.assertIn('delete-keychain',log)
 def test_bash_syntax_and_platform_guard(self):
  subprocess.run(['bash','-n',str(SCRIPT)],check=True)
  env={k:v for k,v in os.environ.items() if not k.startswith('APPLE_')}; r=subprocess.run(['bash',str(SCRIPT),'/tmp/Muninn.app'],env=env,text=True,capture_output=True); self.assertNotEqual(r.returncode,0); self.assertIn('APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64',r.stderr)
if __name__=='__main__': unittest.main()
