#!/usr/bin/env python3
"""Exercise the registered updater and application APIs in a real native app."""
import json, os, pathlib, plistlib, subprocess, sys, time
mode, root_arg, app_arg = sys.argv[1:]
root, app = pathlib.Path(root_arg), pathlib.Path(app_arg)
def executable(bundle):
    info=plistlib.loads((bundle/'Contents/Info.plist').read_bytes())
    return (bundle/'Contents/MacOS'/info['CFBundleExecutable']).resolve()
binary=executable(app)

def bridge(expression):
    ident='updater-'+str(time.time_ns()); d=pathlib.Path(os.environ['MUNINN_E2E_DIR'])
    (d/(ident+'.cmd.json')).write_text(json.dumps({'id':ident,'op':'eval','target':'chrome','js':f'return await ({expression});'}))
    result=d/(ident+'.res.json')
    try:
        for _ in range(400):
            if result.exists(): return json.loads(result.read_text())
            time.sleep(.1)
        raise RuntimeError('native E2E bridge timed out')
    finally:
        (d/(ident+'.cmd.json')).unlink(missing_ok=True); result.unlink(missing_ok=True)
def invoke(name,args=None): return bridge(f'Muninn.invoke({json.dumps(name)},{json.dumps(args or {})})')
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
def wait_bridge(timeout=180):
    end=time.monotonic()+timeout; last=None
    while time.monotonic()<end:
        try: last=invoke('updater_status'); return last
        except (RuntimeError, OSError, ValueError): time.sleep(.25)
    raise RuntimeError(f'updater relaunch bridge timeout; last response {last}')
old=json.loads((root/'old.json').read_text())['version']; new=json.loads((root/'new.json').read_text())['version']
if mode=='healthy':
    # Isolated, pristine starting app. Installation replaces only this copy.
    app=root/'healthy-app'; binary=executable(app)
proc=launch(old)
try:
    time.sleep(3)
    if mode=='healthy':
        before=invoke('state',{})
        before_pid=before.get('process_id'); before_exe=before.get('executable_path')
        if not isinstance(before_pid,int) or before_pid<=0 or not before_exe: raise RuntimeError(f'pre-install identity missing: {before}')
        api('PUT','/api/settings',{'theme':'classified_light','offline_mode':False,'auto_prefetch':False,'min_free_bytes':0})
        settings_before=api('GET','/api/settings')
        if settings_before.get('theme')!='classified_light': raise RuntimeError('settings fixture failed')
        marker='updater-smoke-'+str(time.time_ns()); invoke('tab_new',{'url':'https://example.com/'+marker})
        tabs_before=invoke('tabs_state'); matching=[t for t in (tabs_before.get('tabs') or []) if marker in json.dumps(t)]
        if not matching: raise RuntimeError('unique synthetic tab fixture failed')
        invoke('updater_check'); state=wait_state(proc,{'available','unavailable','failed'})
        if state.get('state')!='available': raise RuntimeError(f'signed update unavailable: {state}')
        invoke('updater_download'); state=wait_state(proc,{'ready_to_install','failed'})
        if state.get('state')!='ready_to_install': raise RuntimeError(f'signed archive download failed: {state}')
        invoke('updater_install'); proc.wait(timeout=120)
        # Only the updater may relaunch. Require a fresh native state response and identity.
        after=wait_bridge(); after_state=invoke('state',{})
        after_pid=after_state.get('process_id'); after_exe=after_state.get('executable_path')
        expected_executable=str(executable(app))
        if not isinstance(after_pid,int) or after_pid<=0 or not after_exe: raise RuntimeError(f'post-install identity missing: {after_state}')
        post_settings=api('GET','/api/settings'); post_tabs=invoke('tabs_state')
        post_matching=[t for t in (post_tabs.get('tabs') or []) if marker in json.dumps(t)]
        settings_ok=post_settings.get('theme')==settings_before.get('theme') and post_settings.get('theme')=='classified_light'
        tabs_ok=bool(post_matching) and post_matching[0].get('url')==matching[0].get('url')
        evidence={'signature_accepted':True,'replacement_installed':plistlib.loads((app/'Contents/Info.plist').read_bytes()).get('CFBundleShortVersionString')==new,
          'restart_requested':True,'restart_observed':bool(after),'restart_process_exited':proc.poll() is not None,
          'settings_persisted':settings_ok,'tabs_persisted':tabs_ok,
          'restart_identity':{'before_pid':before_pid,'after_pid':after_pid,'expected_executable':expected_executable,'actual_executable':str(pathlib.Path(after_exe).resolve())}}
        (root/'healthy-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all((evidence['replacement_installed'],evidence['restart_observed'],evidence['restart_process_exited'],settings_ok,tabs_ok,after_pid!=before_pid,evidence['restart_identity']['actual_executable']==expected_executable)): raise RuntimeError('post-install assertions failed')
    elif mode=='tampered':
        invoke('updater_check'); state=wait_state(proc,{'available','unavailable','failed'})
        if state.get('state')!='available': raise RuntimeError(f'tamper candidate was not offered as newer: {state}')
        invoke('updater_download'); state=wait_state(proc,{'failed','ready_to_install'})
        reason=state.get('status',{}).get('reason') if isinstance(state.get('status'),dict) else state.get('reason')
        rejected=state.get('state')=='failed' and reason=='signature_verification_failed'
        usable=api('GET','/api/settings') is not None and proc.poll() is None
        evidence={'native_app':proc.poll() is None,'tamper_rejected':rejected,'tamper_status':state,'old_version_retained':plistlib.loads((app/'Contents/Info.plist').read_bytes()).get('CFBundleShortVersionString')==old,'app_usable':usable}
        (root/'tampered-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all((evidence['native_app'],rejected,evidence['old_version_retained'],usable)): raise RuntimeError('tamper rejection assertions failed')
    else: raise RuntimeError('mode must be healthy or tampered')
finally:
    if proc.poll() is None: proc.terminate()
    try: proc.wait(timeout=10)
    except subprocess.TimeoutExpired: proc.kill()
