/* Runs only in the hosted debug WKWebView, with local synthetic search data. */
(function(root) {
  root.__muninnNativeSearchInteractions=async function(tabs) {
    const M=root.Muninn, original={api:M.api,newTab:M.newTab,openVideo:M.openVideo,openChannel:M.openChannel,settings:{...M.settings},state:M.searchState,offline:M.offline};
    const results=[], calls=[], actions=[];
    const check=(name,ok,detail)=>{results.push({name,passed:!!ok,detail:detail??null});if(!ok)throw Error(name+': '+JSON.stringify(detail));};
    const settle=()=>new Promise(resolve=>setTimeout(resolve,220));
    const rows=()=>[...document.querySelectorAll('.search-result')];
    const choose=(label,value)=>{const el=document.querySelector('select[aria-label="'+label+'"]');el.value=value;el.dispatchEvent(new Event('change'));};
    const query=async text=>{const el=document.querySelector('#universal-query');el.value=text;el.dispatchEvent(new Event('input'));await settle();};
    const sources={
      '/api/library':[
        {video_id:'SrchSaved01',title:'Fixture protected',state:'protected',status:'complete',protected:true},
        {video_id:'SrchCache01',title:'Fixture cache',state:'complete',status:'complete',protected:false},
        {video_id:'SrchLinks01',title:'Fixture link',state:'link',saved_link:true},
      ],
      '/api/channels':[{channel_id:'UC-fixture-saved',name:'Fixture channel',source:'browsing'},{channel_id:'UC-fixture-browsed',name:'Fixture incidental',source:'browsing'}],
      '/api/subscriptions':[{channel_id:'UC-fixture-saved',name:'Fixture channel',enabled:false}],
      '/api/channel-groups':[],
    };
    M.api=async function(method,path,body,opts) {
      if(method==='GET' && Object.hasOwn(sources,path)) {calls.push(path);return sources[path];}
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
      M.searchState={query:'Fixture',scope:'all',saved:'all',category:'videos'};
      M.openSearch();await settle();
      check('search focuses input',document.activeElement?.id==='universal-query');
      check('local search reads all local sources',calls.length===4,calls);
      check('local typing never launches remote search',actions.length===0,actions);
      check('saved and unsaved are both explicit',rows().filter(n=>n.dataset.searchKind==='library').map(n=>n.querySelector('.badge')?.textContent).join('|')==='Saved|Not saved|Saved');
      check('incidental channels are excluded',rows().filter(n=>n.dataset.searchKind==='channels').length===1);
      choose('Search in','library');choose('Library saved status','unsaved');
      check('not-saved filter selects disposable cache',rows().length===1&&rows()[0].dataset.id==='SrchCache01');
      rows()[0].click();await settle();check('library result opens the selected video',actions.at(-1)?.[1]==='SrchCache01',actions);
      choose('Library saved status','saved');check('saved filter includes protected media and saved links',rows().length===2);
      await query('no-local-result');check('no-results explanation is actionable',document.querySelector('.search-results').textContent.includes('No local matches'));
      check('no-results does not launch online search',actions.length===1);
      for(const category of ['channels','videos','live']) {
        choose('YouTube category',category);document.querySelector('#search-youtube').click();await settle();
        const url=new URL(actions.at(-1)[1]);
        check('explicit online '+category+' filter',url.searchParams.get('sp')==={channels:'EgIQAg==',videos:'EgIQAQ==',live:'EgJAAQ=='}[category],url.href);
      }
      M.settings.offline_mode=true;await query('offline');check('offline search blocks live request',document.querySelector('#search-youtube').disabled);
      M.settings.offline_mode=false;choose('Search in','tabs');await query('');
      const tabRow=rows().find(n=>Number(n.dataset.id)===tabs[0]);check('open tab is searchable',!!tabRow);
      tabRow.click();await settle();check('tab result activates existing native tab',M.tabs.active===tabs[0],M.tabs.active);
      return {passed:true,results};
    } finally {
      M.api=original.api;M.newTab=original.newTab;M.openVideo=original.openVideo;M.openChannel=original.openChannel;
      M.searchState=original.state;M.settings=original.settings;M.offline=original.offline;M.applyAppearance();
      await M.setTabLayout(original.settings.tab_layout||'horizontal');
    }
  };
})(window);
