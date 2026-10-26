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
    session = send({'op':'youtube_cookie_fixture'})
    check('native downloader reads shared cookies before page observation and follows rotation/sign-out', session.get('ok') is True, session)
    click('#sidebar [data-page="library"]')
    ev("(()=>{window.dispatchEvent(new KeyboardEvent('keydown',{key:',',metaKey:true,bubbles:true,cancelable:true}));return true})()")
    check('Cmd comma fallback opens Settings',ev("window.Muninn.page")=='settings')
    shortcut_tab=send({'op':'call','name':'tab_new','args':{'url':'https://www.youtube.com/'}})['id']
    deadline=time.monotonic()+10
    while str(ev("window.Muninn.tabs.active"))!=str(shortcut_tab):
        if time.monotonic()>deadline: raise TimeoutError('shortcut fixture tab did not activate')
        time.sleep(.1)
    ev("(()=>{window.Muninn.page=null;window.Muninn.render();return true})()")
    deadline=time.monotonic()+10
    while True:
        shortcut=send({'op':'settings_menu','tab_id':shortcut_tab})
        if shortcut.get('ok'): break
        if time.monotonic()>deadline: raise AssertionError(shortcut)
        time.sleep(.2)
    deadline=time.monotonic()+10
    while ev("window.Muninn.page")!='settings':
        if time.monotonic()>deadline: raise TimeoutError('native Settings action did not open Settings')
        time.sleep(.1)
    check('native Settings accelerator and menu action work with YouTube focus',shortcut.get('ok'),shortcut)
    session_status=send({'op':'eval','target':'chrome','js':"return await window.Muninn.invoke('youtube_session_status');"})
    check('session status IPC reads the saved store and exposes only presence',isinstance(session_status,dict) and set(session_status)=={'state'} and session_status['state'] in ('present','absent'),session_status)
    send({'op':'call','name':'tab_close','args':{'id':shortcut_tab}})
    for page in ('settings','library'):
        click(f'#sidebar [data-page="{page}"]')
        state=ev("({pageOpen:document.querySelector('#app').dataset.pageOpen, selected:[...document.querySelectorAll('#sidebar [aria-current=\\\"page\\\"]')].map(x=>x.dataset.page), content:document.querySelector('#content').innerText.slice(0,120)})")
        check('sidebar '+page, state.get('selected')==[page],state)
        check('page shown '+page, state.get('pageOpen')=='1',state)
        if page=='settings':
            deadline=time.monotonic()+10
            while not ev("!!document.querySelector('#set-theme')"):
                if time.monotonic()>deadline: raise TimeoutError('settings fields did not load')
                time.sleep(.1)
            layout=ev("(()=>{const f=document.querySelector('.settings-feedback'), n=document.querySelector('#set-favourite_carousel_days');return {feedbackPosition:getComputedStyle(f).position,numberWidth:n.getBoundingClientRect().width,channel:document.querySelector('#update-channel').value}})()")
            check('settings feedback stays in flow and numeric fields are bounded',layout['feedbackPosition'] not in ('sticky','fixed') and layout['numberWidth']<=160 and layout['channel']=='stable',layout)
            screenshot('settings')
            for theme in ('classified','classified_light','system'):
                ev(f"(()=>{{window.Muninn.settings.theme={json.dumps(theme)};window.Muninn.applyAppearance();const n=document.querySelector('#set-favourite_carousel_days'),v=document.querySelector('#view-internal');v.scrollTop+=n.getBoundingClientRect().top-v.getBoundingClientRect().top-8;return true}})()")
                hit=ev("(()=>{const n=document.querySelector('#set-favourite_carousel_days'),b=n.getBoundingClientRect(),hit=document.elementFromPoint(b.left+b.width/2,b.top+8);return {uncovered:hit===n,covering:hit?.className}})()")
                check('scrolled settings input is not covered '+theme,hit['uncovered'],hit)
                screenshot('settings-scroll-'+theme)
            ev("(()=>{document.querySelector('#view-internal').scrollTop=0;return true})()")
    hover=ev("(()=>{const e=document.querySelector('#sidebar [data-page=\\\"settings\\\"]');if(!e)throw Error('missing settings');const before=e.getAttribute('aria-current');e.dispatchEvent(new MouseEvent('mouseenter',{bubbles:false,clientX:4,clientY:4}));return {before,after:e.getAttribute('aria-current'),className:e.className}})()")
    check('synthetic OS-hover does not select settings',hover['before']==hover['after'],hover)
    for expanded in (False, True):
        now=ev("document.querySelector('#side-toggle').getAttribute('aria-expanded')")
        wanted='true' if expanded else 'false'
        if now != wanted: click('#side-toggle')
        state=ev("({expanded:document.querySelector('#side-toggle').getAttribute('aria-expanded'), width:document.querySelector('#sidebar').getBoundingClientRect().width, appSidebar:document.querySelector('#app').dataset.sidebar})")
        check('sidebar state '+wanted,state['expanded']==wanted and state['width']>0,state)
    screenshot('rail')
    for theme in ('classified','classified_light','system'):
        ev(f"(()=>{{window.Muninn.settings.theme={json.dumps(theme)};window.Muninn.applyAppearance();return true}})()")
        click('#side-toggle')
        bounds=ev("(()=>{const rail=document.querySelector('#sidebar').getBoundingClientRect();return [...document.querySelectorAll('.side-nav .side-item')].filter(e=>!e.hidden).map(e=>{const b=e.getBoundingClientRect(),i=e.querySelector('.side-icon').getBoundingClientRect();return {fits:b.left>=rail.left&&b.right<=rail.right&&i.left>=b.left&&i.right<=b.right,width:b.width}})})()")
        check('collapsed navigation fits '+theme,all(b['fits'] and b['width']>=32 for b in bounds),bounds)
        screenshot('rail-'+theme)
        click('#side-toggle')
    for theme in ('dark','light'):
        state=ev(f"(()=>{{window.Muninn.settings.theme={json.dumps(theme)};window.Muninn.applyAppearance();return {{theme:document.documentElement.dataset.theme,computed:getComputedStyle(document.documentElement).colorScheme}}}})()")
        check('appearance '+theme,state['theme']==theme,state)
    tabs=[]
    # Distinct canonical destinations exercise production find-or-open
    # deduplication: each query is meaningful (not stripped tracking), and the
    # run marker prevents an existing/restored tab from absorbing either one.
    # so an existing/restored tab cannot absorb either synthetic tab.
    run_marker=f'native-smoke-{os.getpid()}-{time.time_ns()}'
    fixture_urls=[f'https://www.youtube.com/?native_smoke={run_marker}-1',
                  f'https://www.youtube.com/?native_smoke={run_marker}-2']
    for fixture_url in fixture_urls:
        r=send({'op':'call','name':'tab_new','args':{'url':fixture_url}})
        if not r.get('ok'): raise RuntimeError(r)
        tabs.append(r['id'])
        # Wait for each native tab to appear before creating the next; tab_new's
        # returned ID is the authoritative identity, not a synthetic assumption.
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            state=ev("({ids:window.Muninn.tabs.list.map(t=>t.id)})")
            if str(r['id']) in [str(i) for i in state['ids']]: break
            time.sleep(.05)
        else: raise TimeoutError(f"new tab {r['id']} did not reach UI state: {state!r}")
    check('synthetic tabs have distinct native IDs',len(set(map(str,tabs)))==2,tabs)
    report['synthetic_tabs']=tabs
    def tabstate(): return ev("({active:window.Muninn.tabs.active,count:window.Muninn.tabs.list.length,focusedTab:document.activeElement?.closest('.tab')?.dataset.id ?? null,buttons:[...document.querySelectorAll('#tabstrip .tab-main')].map(x=>({id:x.parentNode.dataset.id,selected:x.getAttribute('aria-selected')}))})")
    base=tabstate(); check('synthetic tabs setup',base['count']>=2 and all(str(i) in [str(b['id']) for b in base['buttons']] for i in tabs),base)
    def select_and_sync(tab_id):
        ev(f"(()=>{{const n=[...document.querySelectorAll('#tabstrip .tab-main')].find(x=>Number(x.parentNode.dataset.id)==={int(tab_id)});if(!n)throw Error('missing tab '+{int(tab_id)});n.focus();n.click();return true}})()")
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            state=tabstate()
            if str(state['active'])==str(tab_id) and str(state['focusedTab'])==str(tab_id) and any(str(b['id'])==str(tab_id) and b['selected']=='true' for b in state['buttons']): return state
            time.sleep(.05)
        raise TimeoutError(f'tab {tab_id} activation/render/focus did not synchronize: {state!r}')
    # Probe unmodified keys from a separately selected/focused tab: they must
    # leave active ID and count alone regardless of the earlier setup focus.
    base=select_and_sync(tabs[0]); check('tab selection synchronizes active render and focus',True,base)
    for key in ('ArrowLeft','ArrowRight','Home','End','Delete','Backspace'):
        ev(f"(()=>{{const n=document.querySelector('#tabstrip .tab-main[aria-selected=\\\"true\\\"]');n.dispatchEvent(new KeyboardEvent('keydown',{{key:{json.dumps(key)},bubbles:true,cancelable:true}}));return true}})()")
        after=tabstate(); check('plain '+key+' leaves tab state unchanged',after['active']==base['active'] and after['count']==base['count'],after)
    screenshot('key')
    for shift in (False,True):
        before=select_and_sync(tabs[0])
        visible=[int(b['id']) for b in before['buttons']]
        index=visible.index(int(before['active']))
        delta=-1 if shift else 1
        expected=visible[(index+delta)%len(visible)]
        ev(f"(()=>{{const n=document.querySelector('#tabstrip .tab-main[aria-selected=\\\"true\\\"]');n.dispatchEvent(new KeyboardEvent('keydown',{{key:'Tab',ctrlKey:true,shiftKey:{str(shift).lower()},bubbles:true,cancelable:true}}));return true}})()")
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            after=tabstate()
            if str(after['active'])==str(expected) and str(after['focusedTab'])==str(expected): break
            time.sleep(.05)
        check('Ctrl'+('Shift+' if shift else '+')+'Tab activates adjacent tab',str(after['active'])==str(expected) and str(after['focusedTab'])==str(expected),{'before':before,'after':after,'expected':expected})
    for op,key in [('search_menu','K'),('address_menu','L')]:
        send({'op':'eval','target':'chrome','js':'return await window.Muninn.activateTab('+str(tabs[0])+');'})
        before=ev('window.Muninn.tabs.list.length')
        native_search=send({'op':op,'tab_id':tabs[0]})
        check('native Cmd+'+key+' binding and action',native_search.get('ok') is True,native_search)
        time.sleep(.3)
        focus=ev("({page:window.Muninn.page,focused:document.activeElement?.id,count:window.Muninn.tabs.list.length})")
        check('native Cmd+'+key+' opens and focuses search without adding tabs',focus['page']=='search' and focus['focused']=='universal-query' and focus['count']==before,focus)
    fixture=pathlib.Path(__file__).with_name('native-search-interactions.js').read_text()
    install=ev('(()=>{'+fixture+';return typeof window.__muninnNativeSearchInteractions})()')
    check('search fixture installed',install=='function',install)
    search=send({'op':'eval','target':'chrome','js':'return await window.__muninnNativeSearchInteractions('+json.dumps(tabs)+');'},timeout=60)
    report['search']=search
    for row in search.get('results',[]): check('search '+row['name'],row.get('passed') is True,row.get('detail'))
    check('search fixture complete',search.get('passed') is True,search)
    screenshot('search')
    media=os.environ.get('NATIVE_SMOKE_MEDIA_FILE')
    if not media or not pathlib.Path(media).is_file(): raise RuntimeError('NATIVE_SMOKE_MEDIA_FILE missing; workflow must generate a synthetic playable media file')
    payload=base64.b64encode(pathlib.Path(media).read_bytes()).decode('ascii')
    fixture=pathlib.Path(__file__).with_name('native-player-interactions.js').read_text()
    install=ev('(()=>{'+fixture+';return typeof window.__muninnNativePlayerInteractions})()')
    check('async player fixture installed',install=='function',install)
    js="return await window.__muninnNativePlayerInteractions({playlistId:'native-fixture-list',mediaBase64:'"+payload+"'})"
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
