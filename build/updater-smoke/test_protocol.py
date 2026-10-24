"""Host-independent checks for updater evidence contract and fail-closed negatives."""
import copy,json,pathlib,subprocess,sys,tempfile,unittest
HERE=pathlib.Path(__file__).resolve().parent
BASE_H={'signature_accepted':True,'replacement_installed':True,'restart_requested':True,'restart_observed':True,'restart_process_exited':True,'settings_persisted':True,'tabs_persisted':True,'restart_identity':{'before_pid':101,'after_pid':202,'expected_executable':'/tmp/Muninn.app/Contents/MacOS/muninn','actual_executable':'/tmp/Muninn.app/Contents/MacOS/muninn'}}
BASE_T={'tamper_rejected':True,'tamper_status':{'state':'failed','status':{'reason':'signature_verification_failed'}},'old_version_retained':True,'app_usable':True}
def run(root): return subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),str(root)],capture_output=True)
class EvidenceProtocol(unittest.TestCase):
 def test_driver_state_identity_keys_match_native_schema(self):
  driver=(HERE/'driver.py').read_text()
  schema=(HERE.parent.parent/'src-tauri/src/e2e.rs').read_text()
  self.assertIn('state["process_id"]',schema)
  self.assertIn("before.get('process_id')",driver)
  self.assertIn("after_state.get('process_id')",driver)
  self.assertIn("before.get('executable_path')",driver)
  self.assertIn("after_state.get('executable_path')",driver)
  self.assertNotIn('has_process_id',driver)
 def test_missing_evidence_fails_closed(self):
  with tempfile.TemporaryDirectory() as d:
   result=run(d); self.assertNotEqual(result.returncode,0); self.assertIn(b'FAIL',result.stderr)
 def test_complete_evidence_and_negative_identity_and_reason(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d); h=copy.deepcopy(BASE_H); t=copy.deepcopy(BASE_T)
   def write():
    (root/'healthy-result.json').write_text(json.dumps(h)); (root/'tampered-result.json').write_text(json.dumps(t))
   write(); subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),d],check=True)
   emitted=json.loads((root/'evidence.json').read_text()); self.assertEqual(emitted,{'healthy':h,'tampered':t})
   for key,value in [('after_pid',101),('actual_executable','/wrong/app')]:
    bad=copy.deepcopy(h); bad['restart_identity'][key]=value
    (root/'healthy-result.json').write_text(json.dumps(bad)); self.assertNotEqual(run(root).returncode,0)
   for reason in ('signature failed','different_reason'):
    bad=copy.deepcopy(t); bad['tamper_status']['status']['reason']=reason
    (root/'healthy-result.json').write_text(json.dumps(h)); (root/'tampered-result.json').write_text(json.dumps(bad))
    self.assertNotEqual(run(root).returncode,0)
if __name__=='__main__': unittest.main()
