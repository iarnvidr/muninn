"""Replay the smoke drivers' actual bridge calls without a native window."""
import ast
import json
import os
import pathlib
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def driver_functions(*names):
    tree = ast.parse((ROOT / 'build/updater-smoke/driver.py').read_text())
    functions = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
    namespace = dict(json=json, os=os, pathlib=pathlib, time=time)
    exec(compile(ast.Module(body=functions, type_ignores=[]), 'driver.py', 'exec'), namespace)
    return namespace


class NativeDriverContracts(unittest.TestCase):
    def test_updater_bridge_returns_settings_and_propagates_native_errors(self):
        functions = driver_functions('bridge')
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ, MUNINN_E2E_DIR=directory), patch.object(time, 'time_ns', return_value=123):
                result = Path(directory) / 'updater-123.res.json'
                result.write_text(json.dumps({'theme': 'classified_light'}))
                self.assertEqual(functions['bridge']('Muninn.api("GET","/api/settings")'), {'theme': 'classified_light'})
                self.assertFalse(result.exists())
                result.write_text(json.dumps({'error': 'HTTP 401'}))
                with self.assertRaisesRegex(RuntimeError, 'native E2E command failed: HTTP 401'):
                    functions['bridge']('Muninn.api("GET","/api/settings")')
                self.assertFalse(result.exists())

    def test_settings_readiness_waits_for_authenticated_api(self):
        functions = driver_functions('wait_settings')
        responses = iter([RuntimeError('No access token'), None, {'theme': 'system'}])
        def api(*args):
            self.assertEqual(args, ('GET', '/api/settings'))
            value = next(responses)
            if isinstance(value, Exception):
                raise value
            return value
        functions['api'] = api
        with patch.object(time, 'sleep') as sleep:
            self.assertEqual(functions['wait_settings'](), {'theme': 'system'})
            self.assertEqual(sleep.call_count, 2)

    def test_native_player_driver_options_pass_real_fixture_id_validation(self):
        tree = ast.parse((ROOT / 'build/native-ui-driver.py').read_text())
        assignment = next(node for node in ast.walk(tree)
                          if isinstance(node, ast.Assign)
                          and any(isinstance(target, ast.Name) and target.id == 'js' for target in node.targets)
                          and '__muninnNativePlayerInteractions' in ast.unparse(node.value))
        expression = eval(compile(ast.Expression(assignment.value), 'native-ui-driver.py', 'eval'), {'payload': 'AA=='})
        fixture = (ROOT / 'build/native-player-interactions.js').read_text()
        script = '''
const vm = require('node:vm');
const context = {Muninn:{api(){},watch:{}}, atob:x=>Buffer.from(x,'base64').toString('binary'),
  Uint8Array, URL:{}, Blob:class {constructor(){throw Error('media payload decoded');}}};
context.window=context;
vm.runInNewContext(FIXTURE,context);
(async()=>{try {await new Function('window', 'return (async()=>{'+EXPRESSION+'})()')(context);process.exit(1);}
catch(error){if(error.message!=='media payload decoded'){console.error(error);process.exit(1);}}})();
'''.replace('FIXTURE', json.dumps(fixture)).replace('EXPRESSION', json.dumps(expression))
        result = subprocess.run(['node', '-e', script], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
