/* Runs only in the hosted debug WKWebView, with local synthetic search data. */
(function(root) {
  root.__muninnNativeSearchInteractions=async function(tabs) {
    const M=root.Muninn, original={api:M.api,newTab:M.newTab,openVideo:M.openVideo,openChannel:M.openChannel,settings:{...M.settings},state:M.searchState,offline:M.offline};
    const results=[], calls=[], actions=[], onlineCalls=[], delayed=[];
    const check=(name,ok,detail)=>{results.push({name,passed:!!ok,detail:detail??null});if(!ok)throw Error(name+': '+JSON.stringify(detail));};
    const settle=()=>new Promise(resolve=>setTimeout(resolve,220));
    const rows=(band=1)=>[...document.querySelectorAll('[data-band="'+band+'"].search-columns .search-result')];
    const choose=(key,value)=>document.querySelector('[data-filter="'+key+'"] [data-value="'+value+'"]').click();
    const key=value=>{const event=new KeyboardEvent('keydown',{key:value,bubbles:true,cancelable:true});document.activeElement.dispatchEvent(event);return event;};
    const query=async text=>{const el=document.querySelector('#universal-query');el.value=text;el.dispatchEvent(new Event('input'));await settle();};
    const sources={
      '/api/library':[
        {video_id:'SrchSaved01',title:'Fixture protected',channel:'Fixture creator',duration_seconds:123,state:'protected',status:'complete',protected:true},
        {video_id:'SrchCache01',title:'Fixture cache',state:'complete',status:'complete',protected:false},
        {video_id:'SrchLinks01',title:'Fixture link',state:'link',saved_link:true},
      ],
      '/api/channels':[{channel_id:'UC-fixture-saved',name:'Fixture channel',source:'browsing'},{channel_id:'UC-fixture-browsed',name:'Fixture incidental',source:'browsing'}],
      '/api/subscriptions':[{channel_id:'UC-fixture-saved',name:'Fixture channel',enabled:false}],
      '/api/channel-groups':[],
    };
    M.api=async function(method,path,body,opts) {
      if(method==='GET' && Object.hasOwn(sources,path)) {calls.push(path);return sources[path];}
      if(method==='POST'&&path==='/api/search') {
        onlineCalls.push({...body});
        const items=body.category==='channels'?[{id:'UCabcdefghijklmnopqrstuv',kind:'channel',title:body.query+' channel',channel:'Online'}]:
          [{id:body.category==='live'?'SrchLive001':'SrchSaved01',kind:'video',title:body.query+' '+body.category,channel:'Online',duration_seconds:321,live:body.category==='live'},
            {id:'SrchCache01',kind:'video',title:body.query+' second',channel:'Online'}];
        if(body.query==='slow')return new Promise(resolve=>delayed.push(()=>resolve({items:items.map(row=>({...row,title:'STALE'}))})));
        return {items};
      }
      return original.api(method,path,body,opts);
    };
    M.newTab=async url=>{actions.push(['live',url]);return 0;};
    M.openVideo=async id=>{actions.push(['video',id]);};
    M.openChannel=async id=>{actions.push(['channel',id]);};
    try {
      // Persisted layout is exercised through the production settings API.
      for(const layout of ['vertical','horizontal']) {
        await M.setTabLayout(layout);await settle();
        const app=document.querySelector('#app'), rail=document.querySelector('.tabrow').getBoundingClientRect(), content=document.querySelector('#content').getBoundingClientRect();
        const nodes=[...document.querySelectorAll('#tabstrip .tab')].filter(n=>tabs.includes(Number(n.dataset.id)));
        check(layout+' preference applied',app.dataset.tabsLayout===layout && M.settings.tab_layout===layout,{layout:app.dataset.tabsLayout});
        check(layout+' tabs do not overlap content',layout==='vertical'?content.left>=rail.right-1:content.top>=rail.bottom-1,{content:content.toJSON(),rail:rail.toJSON()});
        check(layout+' tab controls fit',nodes.length===2&&nodes.every(n=>{const a=n.getBoundingClientRect(),b=n.querySelector('.tab-close').getBoundingClientRect();return b.left>=a.left&&b.right<=a.right&&b.top>=a.top&&b.bottom<=a.bottom;}),nodes.length);
        if(layout==='vertical') check('vertical tabs stack',nodes[1].getBoundingClientRect().top>=nodes[0].getBoundingClientRect().bottom-1);
        await M.activateTab(tabs[0]);await settle();
        const buttons=[...document.querySelectorAll('#tabstrip .tab-main')],active=buttons.findIndex(b=>Number(b.parentNode.dataset.id)===tabs[0]);
        const expected=Number(buttons[(active+1)%buttons.length].parentNode.dataset.id);
        buttons[active].focus();buttons[active].dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',ctrlKey:true,bubbles:true,cancelable:true}));await settle();
        check(layout+' Ctrl+Tab activates adjacent tab',M.tabs.active===expected,{active:M.tabs.active,expected});
      }
      window.muninnOpenSettings();
      M.settings.offline_mode=false;M.offline=false;
      M.searchState={query:'Fixture',scope:'all',saved:'all',onlineType:'all'};
      M.openSearch();await settle();
      check('search focuses input',document.activeElement?.id==='universal-query');
      check('local search reads all local sources',calls.length===4,calls);
      check('local typing never launches remote search',actions.length===0&&onlineCalls.length===0,{actions,onlineCalls});
      check('filters are visible side buttons without dropdowns',!document.querySelector('.search-page select')&&document.querySelectorAll('.search-filters [aria-pressed]').length===11);
      const online=document.querySelector('[aria-label="Online results"]'),local=document.querySelector('[aria-label="Local results"]'),rail=document.querySelector('.search-filters');
      const assertLayout=label=>{
        const a=online.getBoundingClientRect(), b=local.getBoundingClientRect(), field=document.querySelector('#universal-query').getBoundingClientRect();
        check(label+' online above query and local below query',a.bottom<=field.top&&b.top>=field.bottom&&rail.getBoundingClientRect().right<=a.left,{online:a.toJSON(),field:field.toJSON(),local:b.toJSON()});
        check(label+' cards stay inside their columns',[...document.querySelectorAll('.search-result')].every(n=>{const r=n.getBoundingClientRect(),c=n.closest('.search-column').getBoundingClientRect();return r.left>=c.left&&r.right<=c.right+1&&!!n.querySelector('.search-media')&&!!n.querySelector('.search-creator');}));
      };
      assertLayout('initial');
      check('saved and unsaved are both explicit',rows().filter(n=>n.dataset.searchKind==='library').map(n=>n.querySelector('.badge')?.textContent).join('|')==='Saved|Not saved|Saved');
      check('incidental channels are excluded',rows().filter(n=>n.dataset.searchKind==='channels').length===1);
      const typing=key('j');check('hjkl stays normal text in query input',!typing.defaultPrevented&&document.activeElement.id==='universal-query');
      key('ArrowDown');check('Down from query enters local results before online submission',document.activeElement.dataset.band==='1');
      key('j');check('j moves to next local result',document.activeElement.dataset.id==='SrchCache01');
      key('l');check('l moves to the next local type',document.activeElement.dataset.searchKind==='channels');
      key('h');check('h returns to previous local type',document.activeElement.dataset.searchKind==='library');
      key('Escape');check('Escape returns to query',document.activeElement.id==='universal-query');
      choose('scope','library');choose('saved','unsaved');
      check('not-saved filter selects disposable cache',rows().length===1&&rows()[0].dataset.id==='SrchCache01');
      rows()[0].click();await settle();check('library result opens the selected video',actions.at(-1)?.[1]==='SrchCache01',actions);
      choose('saved','saved');check('saved filter includes protected media and saved links',rows().length===2);
      await query('no-local-result');check('no-results explanation is actionable',local.textContent.includes('No local matches'));
      check('no-results does not launch online search',actions.length===1);
      choose('scope','all');choose('saved','all');await query('Fixture');
      document.querySelector('#universal-query').focus();document.querySelector('.search-query').requestSubmit();await settle();
      check('Enter fetches all three online categories inside search',onlineCalls.length===3&&['videos','live','channels'].every(k=>onlineCalls.some(c=>c.category===k)),onlineCalls);
      check('online results rendered without opening a browser tab',rows(0).length===5&&actions.length===1,actions);
      assertLayout('populated');
      check('video cards show duration metadata',rows(0)[0].querySelector('.search-media-label')?.textContent==='5:21');
      check('media-first results show video artwork and channel avatars',!!rows(0)[0].querySelector('.search-media img')&&!!rows(0).at(-1).querySelector('.search-avatar'));
      key('Escape');key('ArrowUp');check('Up from query enters online results',document.activeElement.dataset.band==='0');
      key('Escape');key('ArrowDown');check('Down from query enters local results even with online matches',document.activeElement.dataset.band==='1');
      key('Escape');key('ArrowUp');
      check('first online result receives focus after Enter',document.activeElement.dataset.band==='0'&&document.activeElement.dataset.column==='0'&&document.activeElement.dataset.row==='1');
      key('ArrowRight');check('Right moves to live results',document.activeElement.dataset.column==='1');
      key('ArrowDown');check('Down moves inside the live type',document.activeElement.dataset.row==='2');
      key('l');check('l moves to channels and clamps short columns',document.activeElement.dataset.column==='2'&&document.activeElement.dataset.row==='1');
      key('k');key('j');check('k and j move through headers and results',document.activeElement.dataset.column==='2'&&document.activeElement.dataset.row==='1');
      choose('onlineType','channels');check('side type filter selects a visible online column without fetching',rows(0).length===1&&onlineCalls.length===3);
      rows(0)[0].click();await settle();check('online channel result opens channel',actions.at(-1)?.[0]==='channel'&&actions.at(-1)?.[1]==='UCabcdefghijklmnopqrstuv',actions);
      choose('onlineType','all');rows(0)[0].focus();key('ArrowDown');key('ArrowDown');
      check('Down walks from online results to local band',document.activeElement.dataset.band==='1');
      key('ArrowUp');check('Up returns to online results',document.activeElement.dataset.band==='0');
      const shell=document.querySelector('.search-page'),oldWidth=shell.style.maxWidth;
      shell.style.maxWidth='560px';await settle();assertLayout('narrow');
      rows(0)[0].focus();key('l');check('narrow keyboard focus stays on a media result',!!document.activeElement.closest('.search-result'));
      shell.style.maxWidth=oldWidth;await settle();
      key('Escape');await query('slow');document.querySelector('.search-query').requestSubmit();await settle();
      await query('fast');document.querySelector('.search-query').requestSubmit();await settle();
      for(const resolve of delayed)resolve();await settle();
      check('stale online responses never replace a newer submitted search',rows(0).length===5&&rows(0).every(n=>!n.textContent.includes('STALE')&&n.textContent.includes('fast')));
      await query('typed but not submitted');check('editing hides stale online results without fetching',rows(0).length===0&&onlineCalls.length===9);
      M.settings.offline_mode=true;await query('offline');check('offline search blocks live request',document.querySelector('#search-youtube').disabled);
      document.querySelector('.search-query').requestSubmit();await settle();check('offline Enter does not fetch',onlineCalls.length===9);
      M.settings.offline_mode=false;choose('scope','tabs');await query('');
      const tabRow=rows().find(n=>Number(n.dataset.id)===tabs[0]);check('open tab is searchable',!!tabRow);
      tabRow.click();await settle();check('tab result activates existing native tab',M.tabs.active===tabs[0],M.tabs.active);
      window.muninnOpenSettings();const count=M.tabs.list.length;window.muninnFocusAddress();await settle();
      check('Cmd+L hook opens search from Settings without a new tab',M.page==='search'&&document.activeElement.id==='universal-query'&&M.tabs.list.length===count);
      choose('scope','all');await query('Fixture');document.querySelector('.search-query').requestSubmit();await settle();
      return {passed:true,results};
    } finally {
      M.api=original.api;M.newTab=original.newTab;M.openVideo=original.openVideo;M.openChannel=original.openChannel;
      M.searchState=original.state;M.settings=original.settings;M.offline=original.offline;M.applyAppearance();
      await M.setTabLayout(original.settings.tab_layout||'horizontal');
    }
  };
})(window);
