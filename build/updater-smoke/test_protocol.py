"""Host-independent checks for the smoke runner's evidence contract."""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent

class EvidenceProtocol(unittest.TestCase):
    def test_missing_evidence_fails_closed(self):
        with tempfile.TemporaryDirectory() as d:
            result = subprocess.run([sys.executable, str(HERE/'verify_evidence.py'), d], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'FAIL', result.stderr)

    def test_only_complete_native_assertions_emit_evidence(self):
        healthy = {'signature_accepted':True,'replacement_installed':True,
                   'restart_requested':True,'restart_process_exited':True}
        tampered = {'tamper_rejected':True}
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root/'healthy-result.json').write_text(json.dumps(healthy))
            (root/'tampered-result.json').write_text(json.dumps(tampered))
            subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),d],check=True)
            emitted=json.loads((root/'evidence.json').read_text())
            self.assertEqual(emitted,{**healthy,**tampered})
            tampered['tamper_rejected']=False
            (root/'tampered-result.json').write_text(json.dumps(tampered))
            result=subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),d],capture_output=True)
            self.assertNotEqual(result.returncode,0)

if __name__ == '__main__': unittest.main()
