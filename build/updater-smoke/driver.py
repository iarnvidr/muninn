#!/usr/bin/env python3
"""Exercise the registered updater and application APIs in a real native app."""
import json, os, pathlib, plistlib, subprocess, sys, time
mode, root_arg, app_arg = sys.argv[1:]
root, app = pathlib.Path(root_arg), pathlib.Path(app_arg)
exe = plistlib.loads((app/'Contents/Info.plist').read_bytes())['CFBundleExecutable']
binary = app/'Contents/MacOS'/exe

def bridge(expression):
    ident='updater-'+str(time.time_ns()); d=pathlib.Path(os.environ['MUNINN_E2E_DIR'])
    (d/(ident+'.cmd.json')).write_text(json.dumps({'id':ident,'op':'eval','target':'chrome','js':f'return await ({expression});'}))
    result=d/(ident+'.res.json')
    for _ in range(400):
        if result.exists(): return json.loads(result.read_text())
        time.sleep(.1)
    raise RuntimeError('native E2E bridge timed out')
def invoke(name): return bridge(f'Muninn.invoke({json.dumps(name)})')
def api(method,path,body=None):
    args=f'{json.dumps(method)},{json.dumps(path)}'
    if body is not None: args+=','+json.dumps(body)
    return bridge(f'Muninn.api({args})')
def launch(version):
    env=os.environ.copy(); env['MUNINN_UPDATER_SMOKE_CURRENT_VERSION']=version
    return subprocess.Popen([str(binary),'--native-ui-smoke','--updater-smoke'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def wait_state(proc,wanted,timeout=180):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        if proc.poll() is not None: raise RuntimeError('native app exited prematurely')
        state=invoke('updater_status')
        if state.get('state') in wanted: return state
        time.sleep(.25)
    raise RuntimeError(f'updater status timeout; last state {state}')
def version(): return plistlib.loads((app/'Contents/Info.plist').read_bytes()).get('CFBundleShortVersionString')
old=json.loads((root/'old.json').read_text())['version']; new=json.loads((root/'new.json').read_text())['version']
proc=launch(old)
try:
    time.sleep(3)
    if mode=='healthy':
        api('PUT','/api/settings',{'theme':'dark','offline_mode':False,'auto_prefetch':False,'min_free_bytes':0})
        settings=api('GET','/api/settings')
        if settings.get('theme')!='dark': raise RuntimeError('settings fixture failed')
        tabs=api('GET','/api/tabs')
        invoke('updater_check'); state=wait_state(proc,{'available','unavailable','failed'})
        if state.get('state')!='available': raise RuntimeError(f'signed update unavailable: {state}')
        invoke('updater_download'); state=wait_state(proc,{'ready_to_install','failed'})
        if state.get('state')!='ready_to_install': raise RuntimeError(f'signed archive download failed: {state}')
        invoke('updater_install'); proc.wait(timeout=120)
        # The updater restarts the installed app. Set smoke comparator to the installed release.
        proc=launch(new); time.sleep(5)
        settings_after=api('GET','/api/settings'); tabs_after=api('GET','/api/tabs')
        observed=version()
        evidence={'native_app':proc.poll() is None,'signature_accepted':True,'replacement_installed':observed==new,
                  'relaunched_new_version':proc.poll() is None and observed==new,
                  'settings_persisted':settings_after==settings,'tabs_persisted':tabs_after==tabs}
        (root/'healthy-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all(evidence.values()): raise RuntimeError('post-install app state assertion failed')
        proc.terminate(); proc.wait(timeout=10)
    elif mode=='tampered':
        invoke('updater_check'); state=wait_state(proc,{'available','unavailable','failed'})
        if state.get('state')!='available': raise RuntimeError(f'tamper candidate was not offered as newer: {state}')
        invoke('updater_download'); state=wait_state(proc,{'failed','ready_to_install'})
        failed=state.get('state')=='failed'
        usable=api('GET','/api/settings') is not None and proc.poll() is None
        evidence={'native_app':proc.poll() is None,'tamper_rejected':failed,
                  'old_version_retained':version()==old,'app_usable':usable}
        (root/'tampered-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all(evidence.values()): raise RuntimeError('tamper rejection assertions failed')
    else: raise RuntimeError('mode must be healthy or tampered')
finally:
    if proc.poll() is None: proc.terminate()
    try: proc.wait(timeout=10)
    except subprocess.TimeoutExpired: proc.kill()
