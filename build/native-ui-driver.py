#!/usr/bin/env python3
"""Assertions over the real app's debug WKWebView via its isolated E2E file protocol."""
import base64, json, os, pathlib, subprocess, sys, time

D = pathlib.Path(os.environ['MUNINN_E2E_DIR'])
seq = 0
report = {'status': 'running', 'results': [], 'screenshots': {}, 'synthetic_tabs': []}
EVIDENCE = pathlib.Path(os.environ.get('NATIVE_UI_EVIDENCE_DIR', '/tmp/muninn-debug-ui-evidence'))
SHOTDIR = EVIDENCE / 'screenshots'
SHOTDIR.mkdir(parents=True, exist_ok=True)
def send(cmd, timeout=30):
    global seq
    seq += 1
    cid = f'native{seq:04d}'; cmd['id'] = cid
    tmp, final = D / (cid+'.tmp'), D / (cid+'.cmd.json')
    tmp.write_text(json.dumps(cmd)); tmp.replace(final)
    result = D / (cid+'.res.json'); end = time.monotonic()+timeout
    while time.monotonic() < end:
        if result.exists():
            raw=result.read_text(); result.unlink()
            return json.loads(raw)
        time.sleep(.1)
    raise TimeoutError(f'no result for {cmd!r}')
def ev(js):
    r=send({'op':'evalsync','target':'chrome','js':'return ('+js+');'})
    if isinstance(r,dict) and (r.get('ok') is False or 'error' in r): raise RuntimeError(r)
    return r
def check(name, condition, detail=None):
    row={'name':name,'passed':bool(condition),'detail':detail}
    report['results'].append(row)
    print('PASS' if condition else 'FAIL',name,json.dumps(detail,default=str))
    if not condition: raise AssertionError(f'{name}: {detail!r}')
def click(selector): return ev(f"(()=>{{let e=document.querySelector({json.dumps(selector)}); if(!e) throw Error('missing '+{json.dumps(selector)}); e.click(); return true}})()")
def screenshot(name):
    path=SHOTDIR/(name+'.png')
    p=subprocess.run(['screencapture','-x',str(path)],capture_output=True,text=True)
    if p.returncode or not path.exists(): raise RuntimeError(f'screencapture {name} failed: {p.stderr}')
    report['screenshots'][name]=str(path)

try:
    for page in ('settings','library'):
        click(f'#sidebar [data-page="{page}"]')
        state=ev("({pageOpen:document.querySelector('#app').dataset.pageOpen, selected:[...document.querySelectorAll('#sidebar [aria-current=\\\"page\\\"]')].map(x=>x.dataset.page), content:document.querySelector('#content').innerText.slice(0,120)})")
        check('sidebar '+page, state.get('selected')==[page],state)
        check('page shown '+page, state.get('pageOpen')=='1',state)
        if page=='settings': screenshot('settings')
    hover=ev("(()=>{const e=document.querySelector('#sidebar [data-page=\\\"settings\\\"]');if(!e)throw Error('missing settings');const before=e.getAttribute('aria-current');e.dispatchEvent(new MouseEvent('mouseenter',{bubbles:false,clientX:4,clientY:4}));return {before,after:e.getAttribute('aria-current'),className:e.className}})()")
    check('synthetic OS-hover does not select settings',hover['before']==hover['after'],hover)
    for expanded in (False, True):
        now=ev("document.querySelector('#side-toggle').getAttribute('aria-expanded')")
        wanted='true' if expanded else 'false'
        if now != wanted: click('#side-toggle')
        state=ev("({expanded:document.querySelector('#side-toggle').getAttribute('aria-expanded'), width:document.querySelector('#sidebar').getBoundingClientRect().width, appSidebar:document.querySelector('#app').dataset.sidebar})")
        check('sidebar state '+wanted,state['expanded']==wanted and state['width']>0,state)
    screenshot('rail')
    for theme in ('dark','light'):
        state=ev(f"(()=>{{window.Muninn.settings.theme={json.dumps(theme)};window.Muninn.applyAppearance();return {{theme:document.documentElement.dataset.theme,computed:getComputedStyle(document.documentElement).colorScheme}}}})()")
        check('appearance '+theme,state['theme']==theme,state)
    tabs=[]
    for _ in range(2):
        r=send({'op':'call','name':'tab_new','args':{'url':'about:blank'}})
        if not r.get('ok'): raise RuntimeError(r)
        tabs.append(r['id'])
    report['synthetic_tabs']=tabs
    ev("(()=>{const mains=[...document.querySelectorAll('#tabstrip .tab-main')];if(!mains.length)throw Error('no tabs rendered');mains[0].focus();return true})()")
    def tabstate(): return ev("({active:window.Muninn.tabs.active,count:window.Muninn.tabs.list.length,focused:document.activeElement.closest('.tab')?.dataset.id,buttons:[...document.querySelectorAll('#tabstrip .tab-main')].map(x=>({id:x.parentNode.dataset.id,selected:x.getAttribute('aria-selected')}))})")
    base=tabstate(); check('synthetic tabs setup',base['count']>=2,base)
    for key in ('ArrowLeft','ArrowRight','Home','End','Delete','Backspace'):
        ev(f"(()=>{{const n=document.querySelector('#tabstrip .tab-main');n.focus();n.dispatchEvent(new KeyboardEvent('keydown',{{key:{json.dumps(key)},bubbles:true,cancelable:true}}));return true}})()")
        after=tabstate(); check('plain '+key+' leaves tab state unchanged',after['active']==base['active'] and after['count']==base['count'],after)
    screenshot('key')
    for key,shift,expected in [('Tab',False,tabs[1]),('Tab',True,tabs[0])]:
        before=tabstate()
        ev(f"(()=>{{const n=document.querySelector('#tabstrip .tab-main[aria-selected=\\\"true\\\"]');n.focus();n.dispatchEvent(new KeyboardEvent('keydown',{{key:'Tab',ctrlKey:true,shiftKey:{str(shift).lower()},bubbles:true,cancelable:true}}));return true}})()")
        after=tabstate(); check('Ctrl'+('Shift+' if shift else '+')+'Tab activates adjacent tab',str(after['active'])==str(expected),{'before':before,'after':after,'expected':expected})
    media=os.environ.get('NATIVE_SMOKE_MEDIA_FILE')
    if not media or not pathlib.Path(media).is_file(): raise RuntimeError('NATIVE_SMOKE_MEDIA_FILE missing; workflow must generate a synthetic playable media file')
    payload=base64.b64encode(pathlib.Path(media).read_bytes()).decode('ascii')
    fixture=pathlib.Path(__file__).with_name('native-player-interactions.js').read_text()
    install=ev('(()=>{'+fixture+';return typeof window.__muninnNativePlayerInteractions})()')
    check('async player fixture installed',install=='function',install)
    js="await window.__muninnNativePlayerInteractions({videoId:'native-fixture-video',playlistId:'native-fixture-list',mediaUrl:'data:video/mp4;base64,"+payload+"'})"
    player=send({'op':'eval','target':'chrome','js':js},timeout=60)
    if isinstance(player,dict) and player.get('error'): raise RuntimeError('async native player fixture failed: '+str(player))
    report['player']=player
    for row in player.get('results',[]): check('player '+row['name'],row.get('passed') is True,row.get('detail'))
    check('player fixture complete',player.get('passed') is True,player)
    screenshot('player')
    report['status']='complete'
except Exception as exc:
    report['status']='failed'; report['error']=str(exc)
    print('ERROR',str(exc),file=sys.stderr)
finally:
    out=EVIDENCE/'reports'/'interactions.json'; out.parent.mkdir(parents=True,exist_ok=True)
    out.write_text(json.dumps(report,indent=2,default=str)+'\n')
    print(json.dumps(report,default=str))
if report['status']!='complete': sys.exit(1)
