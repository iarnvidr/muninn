/* Full-page channel policies, groups and inline video catalogues. */
(function () {
  'use strict';
  const M=window.Muninn,h=M.h;
  const label=(text,input,hint)=>h('label',{class:'field'},h('span',{text}),input,hint?h('span',{class:'hint',text:hint}):null);
  const select=(name,options,value)=>{const node=h('select',{'aria-label':name},...options.map(([value,text])=>h('option',{value:String(value),text})));node.value=String(value);return node;};
  const qualities=[['360','360p'],['480','480p'],['720','720p'],['1080','1080p'],['audio','Audio']];
  const retention=[[0,'Until watched'],[1,'1 day'],[7,'7 days'],[30,'30 days'],['keep','Keep']];
  const defaults={quality:'480',retention_days:0,keep:false,enabled:false,poll_minutes:60,max_per_poll:3};
  const videoLists=new Map();
  const lastVideoSource=new Map();

  M.channelsPanel=()=>{
    let active=false,timer,known=[],configs=[],groups=[],settings={},account={state:'unknown',verified:false},groupFilter='all',editChannel=null,signature='',pending=false,deadline=0;
    let queue=Promise.resolve();
    const selected=new Set(),expanded=new Map();
    const root=h('section',{class:'channels-workspace'}),status=h('span',{role:'status',class:'muted channel-feedback'}),discoveryStatus=h('span',{role:'status',class:'muted'});
    const filter=h('input',{type:'search',placeholder:'Search channels','aria-label':'Search channels',oninput:()=>renderTable()});
    const manual=h('input',{placeholder:'Channel ID or /channel/ URL','aria-label':'Add channel URL or ID'});
    const groupName=h('input',{placeholder:'New group','aria-label':'New channel group name',maxlength:'80'});
    const groupNav=h('div',{class:'channel-group-nav','aria-label':'Channel groups'}),policy=h('div',{class:'channel-policy-editor'});
    const table=h('table',{class:'channel-table'}),bulk=h('div',{class:'channel-bulk'});
    const master=h('input',{type:'checkbox','aria-label':'Enable channel downloads'});
    master.addEventListener('change',()=>mutate(async()=>{settings=await M.api('PUT','/api/settings',{auto_prefetch:master.checked});}));
    const load=h('button',{text:'Load subscriptions',onclick:async()=>{
      load.disabled=true;discoveryStatus.textContent='Loading subscriptions…';
      try{await M.api('POST','/api/feeds/refresh',{kind:'channels'});pending=true;deadline=Date.now()+65000;}
      catch(e){discoveryStatus.textContent=M.errText(e);load.disabled=false;}
    }});
    const accountStatus=h('span',{class:'muted',role:'status'});
    const signIn=h('button',{text:'Sign in to YouTube',onclick:async()=>{try{if(account.state==='signed_out')await M.invoke('signin_popup');else M.newTab('https://www.youtube.com/');}catch(e){status.textContent=M.errText(e);}}});
    function renderAccount(){
      accountStatus.textContent=settings.offline_mode?'Offline · cached channels. YouTube account actions are unavailable.':
        account.state==='signed_out'?'Signed out of YouTube · cached channels and local download rules remain available.':
        account.state==='signed_in'?(account.verified?'Signed in to YouTube · subscription lists are cached. Load subscriptions to refresh this account.':'Signed in · account not verified yet. Reload subscriptions to enable YouTube account actions.'):
        'YouTube sign-in has not been checked. Cached channels and public downloads remain available.';
      signIn.hidden=account.state==='signed_in'&&account.verified;signIn.textContent=account.state==='signed_out'?'Sign in to YouTube':'Open YouTube to check sign-in';
      signIn.disabled=!!settings.offline_mode;load.disabled=pending||!!settings.offline_mode||account.state==='signed_out';
    }
    const iconButton=(text,icon,action)=>h('button',{class:'channel-icon',title:text,'aria-label':text,onclick:action},M.icon(icon));
    const groupOf=id=>groups.find(g=>g.channel_ids.includes(id));
    const override=id=>configs.find(c=>c.channel_id===id);
    function channels(){
      const map=new Map(known.map(c=>[c.channel_id,c]));
      for(const c of configs)if(!map.has(c.channel_id))map.set(c.channel_id,c);
      for(const g of groups)for(const id of g.channel_ids)if(!map.has(id))map.set(id,{channel_id:id,name:id});
      return [...map.values()].sort((a,b)=>(a.name||a.channel_id).localeCompare(b.name||b.channel_id));
    }
    function effective(id){const g=groupOf(id);return {...defaults,...g,...(g?.favourite?{enabled:g.enabled&&settings.favourite_downloads!==false}:{}),...override(id),channel_id:id};}
    function mutate(operation){queue=queue.then(async()=>{try{await operation();status.textContent='Saved';render();}catch(e){status.textContent=M.errText(e);M.flashError(e);}});return queue;}
    function saveGroups(next){return M.api('PUT','/api/channel-groups',next).then(value=>{groups=value;});}
    function saveChannel(id,patch){return mutate(async()=>{const name=channels().find(c=>c.channel_id===id)?.name||'';const base=effective(id);const value={channel_id:id,name,quality:base.quality,retention_days:base.retention_days,keep:base.keep,enabled:base.enabled,poll_minutes:base.poll_minutes,max_per_poll:base.max_per_poll,...patch};configs=await M.api('PUT','/api/subscriptions',configs.filter(c=>c.channel_id!==id).concat(value));});}
    function assign(ids,name){return mutate(async()=>{
      await saveGroups(groups.map(g=>({...g,channel_ids:g.channel_ids.filter(id=>!ids.includes(id)).concat(g.name===name?ids:[])})));
      configs=await M.api('PUT','/api/subscriptions',configs.filter(c=>!ids.includes(c.channel_id)));selected.clear();
    });}
    function groupEdit(name,patch){return mutate(async()=>{await saveGroups(groups.map(g=>g.name===name?{...g,...patch}:g));if(patch.name)groupFilter=patch.name;});}
    function addManual(){
      const id=/UC[A-Za-z0-9_-]{22}/.exec(manual.value)?.[0];
      if(!id){status.textContent='Enter a 24-character UC ID or /channel/ URL.';return;}
      if(!known.some(c=>c.channel_id===id))known.push({channel_id:id,name:id});
      manual.value='';saveChannel(id,{enabled:false});editChannel=id;
    }
    function renderGroups(){
      groupNav.replaceChildren(...[['all','All channels'],['ungrouped','Ungrouped'],...groups.map(g=>[g.name,g.name+' · '+g.channel_ids.length])].map(([key,text])=>h('button',{class:'chip',text,'aria-pressed':String(groupFilter===key),onclick:()=>{groupFilter=key;editChannel=null;render();}})));
    }
    function renderPolicy(){
      policy.replaceChildren();
      const group=groups.find(g=>g.name===groupFilter),id=editChannel,c=id?effective(id):group;
      if(!c)return;
      const name=id?(channels().find(ch=>ch.channel_id===id)?.name||id):group.name;
      const save=patch=>id?saveChannel(id,patch):groupEdit(group.name,patch);
      const quality=select('Policy quality for '+name,qualities,c.quality);quality.onchange=()=>save({quality:quality.value});
      const expiry=select('Policy retention for '+name,retention,c.keep?'keep':c.retention_days);expiry.onchange=()=>save({keep:expiry.value==='keep',retention_days:expiry.value==='keep'?0:Number(expiry.value)});
      const interval=h('input',{type:'number',min:'5',max:'10080',value:c.poll_minutes,'aria-label':'Check interval for '+name,onchange:event=>{const n=Number(event.target.value);if(Number.isInteger(n)&&n>=5&&n<=10080)save({poll_minutes:n});else status.textContent='Check interval must be 5–10080 minutes.';}});
      const limit=h('input',{type:'number',min:'1',max:'15',value:c.max_per_poll,'aria-label':'New upload limit for '+name,onchange:event=>{const n=Number(event.target.value);if(Number.isInteger(n)&&n>=1&&n<=15)save({max_per_poll:n});else status.textContent='Upload limit must be 1–15.';}});
      const enabled=h('input',{type:'checkbox',checked:c.enabled,onchange:event=>save({enabled:event.target.checked})});
      policy.append(h('div',{class:'policy-heading'},h('strong',{text:id?'Channel override · '+name:'Group rule · '+name}),h('label',{class:'check'},enabled,' Download new uploads')),
        h('div',{class:'channel-policy-fields'},label('Quality',quality),label('Retention',expiry),label('Check every (min)',interval),label('New uploads / check',limit)),
        h('p',{class:'muted policy-explanation',text:id?'This channel overrides its group. Reset the override to inherit future group edits.':'Members inherit this rule unless they have a channel override. Changes save immediately.'}));
      if(id)policy.append(h('button',{text:'Close channel policy',onclick:()=>{editChannel=null;renderPolicy();}}));
      else{
        const favourite=h('input',{type:'checkbox',checked:!!group.favourite,onchange:event=>groupEdit(group.name,{favourite:event.target.checked,...(event.target.checked?{enabled:true}:{})})});
        policy.append(h('label',{class:'check'},favourite,' Favourite group · feature new uploads on Browse'));
        const rename=h('input',{value:group.name,'aria-label':'Rename channel group',maxlength:'80'});
        policy.append(h('div',{class:'actions'},rename,h('button',{text:'Rename group',onclick:()=>groupEdit(group.name,{name:rename.value.trim()})}),h('button',{text:'Remove group',onclick:()=>mutate(async()=>{await saveGroups(groups.filter(g=>g.name!==group.name));groupFilter='all';})})));
      }
    }
    function renderBulk(){
      bulk.hidden=!selected.size;
      const target=select('Group for selected channels',[['','Ungrouped'],...groups.map(g=>[g.name,g.name])],'');
      bulk.replaceChildren(h('strong',{text:selected.size+' selected'}),target,h('button',{text:'Apply group policy',title:'Move selected channels and replace overrides with the group rule',onclick:()=>assign([...selected],target.value)}),h('button',{text:'Clear',onclick:()=>{selected.clear();renderTable();}}));
    }
    function catalogue(c){
      const state=expanded.get(c.channel_id),body=h('section',{class:'channel-videos','aria-label':'Videos from '+(c.name||c.channel_id)});
      if(!state)return body;
      const controls=h('div',{class:'actions'},...[['library','In Muninn'],['uploads','Channel videos']].map(([key,text])=>h('button',{text,'aria-pressed':String(state.source===key),onclick:()=>loadVideos(c,key,false)})),
        iconButton('Refresh videos','reload',()=>loadVideos(c,state.source,false,true)));
      body.append(controls);
      if(state.loading||state.refreshing)body.append(h('p',{role:'status',class:'muted',text:state.items?.length?'Showing cached videos · refreshing…':'Loading videos…'}));
      else if(state.cached)body.append(h('span',{class:'muted',text:state.offline?'Cached · offline':state.stale?'Cached · refresh when online':'Cached channel list'}));
      if(state.error)body.append(h('p',{role:'status',class:'error',text:state.error}),h('button',{text:'Retry videos',onclick:()=>loadVideos(c,state.source,false,true)}));
      const cards=h('div',{class:'channel-video-grid'});
      for(const v of state.items||[]){
        const q=effective(c.channel_id).quality;
        const add=h('button',{text:'Download '+M.L.qLabel(q),onclick:async()=>{
          add.disabled=true;
          try{await M.api('POST','/api/cache',{video_id:v.video_id,quality:q,protect:effective(c.channel_id).keep,title:v.title,channel:c.name,channel_id:c.channel_id,duration_seconds:Math.round(v.duration_seconds||0)||null});add.textContent='Queued';}
          catch(e){add.textContent='Retry download';add.disabled=false;status.textContent=M.errText(e);}
        }});
        cards.append(h('article',{class:'channel-video-card'},h('button',{class:'video-preview',title:'Play '+(v.title||v.video_id),onclick:()=>M.openVideo(v.video_id)},M.thumbImg('/thumb/'+v.video_id)),
          h('strong',{text:v.title||v.video_id}),h('span',{class:'muted',text:state.source==='library'?(v.protected?'Kept · ':'')+M.L.qLabel(v.variant)+' · '+v.status:M.L.fmtDuration(v.duration_seconds)}),
          h('div',{class:'actions'},h('button',{text:'Play',onclick:()=>M.openVideo(v.video_id)}),state.source==='uploads'?add:null)));
      }
      body.append(cards);
      if(!state.loading&&!state.error&&!state.items?.length)body.append(h('p',{class:'muted',text:state.source==='library'?'No copies from this channel in Muninn yet.':'No more channel videos found.'}));
      if(state.next!=null&&!state.loading)body.append(h('button',{text:'Load more videos',onclick:()=>loadVideos(c,state.source,true)}));
      return body;
    }
    async function loadVideos(c,source=lastVideoSource.get(c.channel_id)||'library',append=false,refresh=false){
      lastVideoSource.set(c.channel_id,source);
      const key=c.channel_id+':'+source,old=videoLists.get(key)||{items:[]};
      if(!append&&!refresh&&old.loadedAt&&Date.now()-old.loadedAt<60000&&!old.refreshing){expanded.set(c.channel_id,old);renderTable();return;}
      if(old.loading){expanded.set(c.channel_id,old);renderTable();return;}
      const state={...old,source,loading:true,error:null};
      videoLists.set(key,state);
      expanded.set(c.channel_id,state);renderTable();
      try{
        const result=await M.api('GET','/api/channels/'+encodeURIComponent(c.channel_id)+'/videos?source='+source+'&offset='+(append?old.next||0:0)+(refresh?'&refresh=1':''));
        state.items=(append?old.items:[]).concat(result.items||[]);state.next=result.next_offset;state.loading=false;state.loadedAt=Date.now();
        Object.assign(state,{cached:result.cached,stale:result.stale,offline:result.offline,refreshing:result.refreshing,error:result.refresh_error});
        if(active&&expanded.get(c.channel_id)===state)renderTable();
      }catch(e){state.error=M.errText(e);state.loading=false;if(active)renderTable();}
    }
    function renderTable(){
      const list=channels().filter(c=>(groupFilter==='all'||(groupFilter==='ungrouped'?!groupOf(c.channel_id):groupOf(c.channel_id)?.name===groupFilter))&&((c.name||'')+' '+c.channel_id).toLowerCase().includes(filter.value.toLowerCase()));
      const all=h('input',{type:'checkbox','aria-label':'Select visible channels',checked:!!list.length&&list.every(c=>selected.has(c.channel_id)),onchange:event=>{for(const c of list)event.target.checked?selected.add(c.channel_id):selected.delete(c.channel_id);renderTable();}});
      const rows=[];
      for(const c of list){
        const id=c.channel_id,g=groupOf(id),custom=override(id),value=effective(id),name=c.name||id;
        const membership=select('Group for '+name,[['','Ungrouped'],...groups.map(x=>[x.name,x.name])],g?.name||'');membership.title='Changing group uses its shared download rule';membership.onchange=()=>assign([id],membership.value);
        const download=h('input',{type:'checkbox',checked:value.enabled,'aria-label':'Download new uploads from '+name,onchange:event=>saveChannel(id,{enabled:event.target.checked})});
        const quality=select('Quality for '+name,[...(g?[['inherit','Group · '+M.L.qLabel(g.quality)]]:[]),...qualities],g&&!custom?'inherit':value.quality);
        quality.onchange=()=>quality.value==='inherit'?mutate(async()=>{configs=await M.api('PUT','/api/subscriptions',configs.filter(x=>x.channel_id!==id));}):saveChannel(id,{quality:quality.value});
        const expiry=select('Retention for '+name,retention,value.keep?'keep':value.retention_days);expiry.onchange=()=>saveChannel(id,{keep:expiry.value==='keep',retention_days:expiry.value==='keep'?0:Number(expiry.value)});
        const toggle=iconButton((expanded.has(id)?'Hide':'Show')+' videos from '+name,expanded.has(id)?'collapse':'expand',()=>{if(expanded.has(id)){expanded.delete(id);renderTable();}else loadVideos(c);});toggle.setAttribute('aria-expanded',String(expanded.has(id)));
        const actions=h('div',{class:'channel-row-actions'},iconButton('Edit policy for '+name,'settings',()=>{editChannel=editChannel===id?null:id;renderPolicy();policy.scrollIntoView({block:'nearest',behavior:'smooth'});}));
        if(!c.unsubscribed)actions.append(h('button',{class:'danger',text:'Unsubscribe',title:account.verified&&c.youtube_subscription==='current'?'Unsubscribe on YouTube and stop channel downloads':'Load subscriptions for the current signed-in YouTube account to enable unsubscribe',disabled:!!settings.offline_mode||!account.verified||c.youtube_subscription!=='current',onclick:async event=>{
          const button=event.currentTarget;
          if(!await M.confirm('Unsubscribe from '+name+' on YouTube and stop its automatic downloads in Muninn? Existing videos stay in your library.','Unsubscribe',true))return;
          button.disabled=true;status.textContent='Unsubscribing on YouTube…';
          try{await M.invoke('channel_unsubscribe',{channel_id:id});await refresh(true);status.textContent='Unsubscribed on YouTube';M.refreshFeatured?.(true);}
          catch(e){status.textContent=M.errText(e);button.disabled=false;}
        }}));
        if(custom)actions.append(iconButton('Reset override for '+name,'reload',()=>mutate(async()=>{configs=await M.api('PUT','/api/subscriptions',configs.filter(x=>x.channel_id!==id));})));
        rows.push(h('tr',null,h('td',null,h('input',{type:'checkbox','aria-label':'Select '+name,checked:selected.has(id),onchange:event=>{event.target.checked?selected.add(id):selected.delete(id);renderBulk();}})),
          h('td',null,h('div',{class:'channel-name'},toggle,h('button',{class:'link channel-name-link',text:name,title:'Show videos from '+name,onclick:()=>expanded.has(id)?(expanded.delete(id),renderTable()):loadVideos(c)})),h('span',{class:'channel-policy-source muted',text:(custom?'Channel override':g?'Group rule · '+g.name:'Manual · not downloading')+(c.youtube_subscription==='cached'?' · Cached subscription · account not verified':c.youtube_subscription==='current'?' · YouTube subscription (cached list)':c.unsubscribed?' · Unsubscribed on YouTube':'')})),
          h('td',null,membership),h('td',{class:'channel-auto'},download),h('td',null,quality),h('td',null,expiry),h('td',null,actions)));
        if(expanded.has(id))rows.push(h('tr',{class:'channel-expanded-row'},h('td',{colspan:'7'},catalogue(c))));
      }
      table.replaceChildren(h('thead',null,h('tr',null,h('th',null,all),...['Channel','Group','Download','Quality','Retention',''].map(text=>h('th',{text})))),h('tbody',null,...rows));
      if(!list.length)table.append(h('caption',{text:known.length?'No matching channels. Choose All channels to assign members.':'Load subscriptions or add a channel ID to get started.'}));
      renderBulk();
    }
    function render(){master.checked=settings.auto_prefetch;renderAccount();renderGroups();renderPolicy();renderTable();}
    async function refresh(initial=false){
      try{
        for(const [id,state] of expanded)if(state.refreshing&&!state.loading){const c=channels().find(c=>c.channel_id===id);if(c)loadVideos(c,state.source);}
        const [nextKnown,nextConfigs,nextGroups,nextSettings,discovery]=await Promise.all([M.api('GET','/api/channels'),M.api('GET','/api/subscriptions'),M.api('GET','/api/channel-groups'),M.api('GET','/api/settings'),M.api('GET','/api/channels/status')]);
        if(!active)return;
        const nextAccount=discovery.account||{state:'unknown',verified:false};
        if(!pending&&JSON.stringify(nextAccount)!==JSON.stringify(account))discoveryStatus.textContent=discovery.subscriptions_current?'Cached subscription list · '+M.L.relTime(discovery.updated_at):'Load subscriptions to check the current YouTube account.';
        const next=JSON.stringify([nextKnown.map(c=>[c.channel_id,c.name,c.unsubscribed,c.youtube_subscription]),nextConfigs,nextGroups,nextSettings,nextAccount]);
        if(initial||next!==signature){known=nextKnown;configs=nextConfigs;groups=nextGroups;settings=nextSettings;account=nextAccount;signature=next;render();}
        if(pending){
          discoveryStatus.textContent=discovery.phase==='ready'?'Loaded '+discovery.count+' subscribed channels.':discovery.phase==='empty'?'No subscriptions found for this account. Cached and local channels remain available.':discovery.phase==='needs_sign_in'?'Sign in to YouTube, then reload subscriptions.':discovery.phase==='error'?'Could not refresh subscriptions. Cached channels remain available.':'Loading subscriptions · '+(discovery.count||0)+' found';
          if(['ready','needs_sign_in','empty','error'].includes(discovery.phase)||Date.now()>deadline){pending=false;if(discovery.phase==='error'||Date.now()>deadline)discoveryStatus.textContent='Could not refresh subscriptions. Cached channels remain available.';}
        }
        renderAccount();
      }catch(e){if(active)status.textContent=M.errText(e);}
      if(active)timer=setTimeout(refresh,5000);
    }
    return {
      mount(el){
        active=true;
        const create=h('button',{text:'Create group',onclick:()=>{const name=groupName.value.trim();if(!name)return;mutate(async()=>{await saveGroups(groups.concat({name,channel_ids:[],quality:'1080',retention_days:0,keep:false,enabled:true,poll_minutes:60,max_per_poll:3}));groupFilter=name;groupName.value='';});}});
        root.append(h('header',{class:'channel-workspace-head'},h('div',null,h('h2',{text:'Channels'}),h('p',{class:'muted',text:'Choose what each channel downloads. Group rules stay in sync; channel overrides are explicit.'})),h('label',{class:'check'},master,' Downloads enabled')),
          h('div',{class:'channel-account-state'},accountStatus,signIn),h('div',{class:'channel-toolbar'},filter,load,iconButton('Check channel uploads now','reload',async()=>{if(!channels().some(c=>effective(c.channel_id).enabled)){status.textContent='Enable downloads for a channel or group first.';return;}try{await M.api('POST','/api/subscriptions/poll',{});status.textContent='Checking enabled channels…';}catch(e){status.textContent=M.errText(e);}}),discoveryStatus),
          groupNav,h('div',{class:'channel-create'},groupName,create,manual,h('button',{text:'Add channel',onclick:addManual})),policy,bulk,status,table);
        el.append(root);refresh(true);
      },
      unmount(){active=false;clearTimeout(timer);}
    };
  };

  M.cleanupPanel=()=>{
    let active=false, config;
    const form=h('form',{class:'channel-card'}), message=h('p',{role:'status',class:'muted'});
    const enabled=h('input',{type:'checkbox','aria-label':'Remove played videos automatically'});
    const percent=h('input',{type:'number',min:'50',max:'100',step:'1',required:true,'aria-label':'Mostly played threshold percent'});
    const hours=h('input',{type:'number',min:'0',max:'720',step:'any',required:true,'aria-label':'Hours after completion before cleanup'});
    const save=h('button',{type:'submit',class:'primary',text:'Save cleanup policy'});
    form.append(h('h2',{text:'Watch first. Clear space later.'}),h('p',{class:'muted',text:'New default copies stay until you reach 95% of the video. One day later, their media files are removed. Saved videos stay protected; metadata and watch history remain. Disk limits can still evict temporary copies when space is needed.'}),
      h('label',{class:'check'},enabled,' Remove played videos automatically'),h('div',{class:'fieldgrid'},label('Mostly played (%)',percent),label('Remove media after (hours)',hours)),h('div',{class:'actions'},save),message);
    form.addEventListener('submit',async(event)=>{
      event.preventDefault();
      if(!form.reportValidity()||!config)return;
      save.disabled=true;
      try{config=await M.api('PUT','/api/automation',{...config,cleanup_completed:enabled.checked,completion_percent:Number(percent.value),cleanup_delay_seconds:Math.round(Number(hours.value)*3600)});message.textContent='Cleanup policy saved.';}
      catch(e){message.textContent=M.errText(e);M.flashError(e);}finally{save.disabled=false;}
    });
    return {async mount(el){active=true;el.append(form);try{config=await M.api('GET','/api/automation');if(active){enabled.checked=config.cleanup_completed;percent.value=config.completion_percent;hours.value=config.cleanup_delay_seconds/3600;}}catch(e){M.flashError(e);}},unmount(){active=false;}};
  };
})();
