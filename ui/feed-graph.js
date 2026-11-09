/* Feeds network: real snapshot -> video -> channel links, never inferred recommendations.
 * Kept dependency-free and bounded for the app's offline, embedded UI. */
(function (root) {
  'use strict';
  const NS = 'http://www.w3.org/2000/svg';
  const MAX_SNAPSHOTS = 20, MAX_VIDEOS = 64, MAX_CHANNELS = 18;
  const LABELS = {home:'YouTube Home',subscriptions:'Subscriptions',search:'Search results',
    related:'Related videos',channel:'Channel pages',other:'Other captures'};
  const short = (value, n) => String(value || '').slice(0, n);
  function build(snapshots) {
    const nodes = new Map(), edges = new Map(), seenVideos = new Set(), seenChannels = new Set();
    const rows = (Array.isArray(snapshots) ? snapshots : []).slice(0, MAX_SNAPSHOTS);
    const add = (id, kind, label, extra) => {
      if (!nodes.has(id)) nodes.set(id, {id, kind, label:short(label, 130), ...extra});
    };
    const link = (source, target, relation) => {
      const id = source + '|' + target + '|' + relation;
      if (nodes.has(source) && nodes.has(target)) edges.set(id, {id,source,target,relation});
    };
    for (const snapshot of rows) {
      if (!snapshot || !Number.isSafeInteger(Number(snapshot.id))) continue;
      const kind = LABELS[snapshot.kind] ? snapshot.kind : 'other';
      const sourceId = 'source:' + kind, snapId = 'snapshot:' + snapshot.id;
      add(sourceId, 'source', LABELS[kind], {kindName:kind, explanation:'Source category: '+LABELS[kind]});
      const sourceUrl = typeof snapshot.source_url === 'string' ? short(snapshot.source_url, 280) : '';
      const query = typeof snapshot.query === 'string' ? short(snapshot.query, 140) : '';
      const subtitle = query ? ' · '+query : '';
      add(snapId, 'snapshot', (LABELS[kind] || kind)+subtitle, {
        snapshot_id:snapshot.id, source_url:sourceUrl,
        captured_at:snapshot.captured_at, count:snapshot.count,
        explanation:'Captured '+(sourceUrl || 'from a '+kind+' page (source URL not recorded)')});
      link(sourceId, snapId, 'capture');
      for (const item of (snapshot.items || []).slice(0, 24)) {
        const videoId = String(item.video_id || '');
        if (!/^[A-Za-z0-9_-]{11}$/.test(videoId)) continue;
        const videoNode = 'video:'+videoId;
        if (!nodes.has(videoNode) && seenVideos.size >= MAX_VIDEOS) continue;
        seenVideos.add(videoId);
        add(videoNode, 'video', item.title || videoId,
          {video_id:videoId, channel:item.channel || '', explanation:'Seen in a captured YouTube feed'});
        link(snapId,videoNode,'contains');
        const channelId = String(item.channel_id || '');
        if (/^UC[A-Za-z0-9_-]{22}$/.test(channelId) &&
            (seenChannels.has(channelId) || seenChannels.size < MAX_CHANNELS)) {
          seenChannels.add(channelId);
          const channelNode = 'channel:'+channelId;
          add(channelNode,'channel',item.channel || channelId,
            {channel_id:channelId, explanation:'Channel reported by YouTube'});
          link(channelNode,videoNode,'published');
        }
      }
    }
    const values = [...nodes.values()], links = [...edges.values()];
    const degree = new Map(values.map(n=>[n.id,0]));
    for(const edge of links) { degree.set(edge.source,degree.get(edge.source)+1); degree.set(edge.target,degree.get(edge.target)+1); }
    for(const n of values)n.degree=degree.get(n.id);
    return {nodes:values,edges:links, snapshots:rows.length,
      truncated:(Array.isArray(snapshots)&&snapshots.length>rows.length) || seenVideos.size>=MAX_VIDEOS,
      labels:LABELS};
  }

  function positions(graph, width, height) {
    const columns = {source:95,snapshot:335,video:625,channel:900};
    const result = new Map();
    for(const kind of Object.keys(columns)) {
      const group = graph.nodes.filter(n=>n.kind===kind)
        .sort((a,b)=>b.degree-a.degree || a.id.localeCompare(b.id));
      group.forEach((n,i)=>result.set(n.id,{x:columns[kind],
        y:Math.round(32+(i+0.5)*Math.max(40,height-64)/Math.max(group.length,1))}));
    }
    return result;
  }

  function mount(container, callbacks) {
    const doc=container.ownerDocument, create=(tag,attrs)=>{
      const el=doc.createElementNS(NS,tag);
      for(const [key,value]of Object.entries(attrs||{}))el.setAttribute(key,String(value));
      return el;
    };
    const box=doc.createElement('div');box.className='feed-graph-toolbar';
    const button=(label,click)=>{const b=doc.createElement('button');b.type='button';b.textContent=label;b.addEventListener('click',click);box.append(b);return b;};
    const info=doc.createElement('p');info.className='feed-graph-detail';info.setAttribute('role','status');
    const svg=create('svg',{viewBox:'0 0 1000 640',role:'application',tabindex:'0',
      'aria-label':'Feed connections. Use nodes or the snapshot list to explore captured sources, videos and channels.'});
    let bounds={x:0,y:0,w:1000,h:640},selected=null,graph={nodes:[],edges:[]};
    const drawn=new Map(),lines=new Map();
    const view=()=>svg.setAttribute('viewBox',[bounds.x,bounds.y,bounds.w,bounds.h].join(' '));
    const scale=factor=>{
      const w=Math.max(250,Math.min(2400,bounds.w*factor));
      const h=w*0.64;bounds={x:bounds.x+(bounds.w-w)/2,y:bounds.y+(bounds.h-h)/2,w,h};view();
    };
    button('−',()=>scale(1.25)).setAttribute('aria-label','Zoom out');
    button('+',()=>scale(0.8)).setAttribute('aria-label','Zoom in');
    button('Fit',()=>{bounds={x:0,y:0,w:1000,h:640};view();});
    const legend=doc.createElement('p');legend.className='feed-graph-legend';
    legend.textContent='Source → captured snapshot → shared video ← channel. Every line comes from stored YouTube data.';
    container.replaceChildren(box,svg,legend,info);

    function select(id,notify) {
      selected=id;
      const nearby=new Set([id]);
      for(const e of graph.edges)if(e.source===id||e.target===id){nearby.add(e.source);nearby.add(e.target);}
      for(const [key,g]of drawn) {
        g.classList.toggle('selected',key===id);
        g.classList.toggle('adjacent',key!==id&&nearby.has(key));
        g.classList.toggle('faded',!!id&&!nearby.has(key));
      }
      for(const [key,path]of lines)path.classList.toggle('faded',!!id&&!graph.edges.some(e=>e.id===key&&(e.source===id||e.target===id)));
      const node=graph.nodes.find(n=>n.id===id);
      if(node) {
        info.textContent=node.label+' · '+(node.explanation||'')+
          (node.kind==='snapshot' ? ' · '+(node.count||0)+' videos' : '');
        if(notify)callbacks?.onSelect?.(node);
      } else info.textContent='Select a node to see its actual capture source and connections.';
    }
    function render(data,focusId) {
      graph=data||{nodes:[],edges:[]};drawn.clear();lines.clear();svg.replaceChildren();
      const pos=positions(graph,1000,640);
      for(const edge of graph.edges) {
        const a=pos.get(edge.source),b=pos.get(edge.target);
        if(!a||!b)continue;
        const line=create('path',{d:'M'+a.x+' '+a.y+' C'+((a.x+b.x)/2)+' '+a.y+' '+((a.x+b.x)/2)+' '+b.y+' '+b.x+' '+b.y,
          class:'feed-edge '+edge.relation});
        svg.append(line);lines.set(edge.id,line);
      }
      for(const node of graph.nodes) {
        const p=pos.get(node.id);if(!p)continue;
        const group=create('g',{class:'feed-node '+node.kind,transform:'translate('+p.x+','+p.y+')',
          tabindex:'0',role:'button','aria-label':node.label+'; '+(node.explanation||'')});
        const shape=create(node.kind==='snapshot'?'rect':'circle',node.kind==='snapshot'
          ?{x:-9,y:-9,width:18,height:18,rx:3}:{cx:0,cy:0,r:node.kind==='source'?12:node.kind==='channel'?9:7});
        const title=create('title');title.textContent=node.label+(node.source_url?' · '+node.source_url:'');
        const label=create('text',{x:0,y:node.kind==='source'?-18:24,'text-anchor':'middle'});
        label.textContent=short(node.label,node.kind==='source'?22:19);
        group.append(shape,title,label);
        group.addEventListener('click',()=>select(node.id,true));
        group.addEventListener('dblclick',()=>callbacks?.onOpen?.(node));
        group.addEventListener('keydown',e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();select(node.id,true);}});
        svg.append(group);drawn.set(node.id,group);
      }
      select(focusId&&drawn.has(focusId)?focusId:null,false);
      if(!graph.nodes.length)info.textContent='No captured feed connections in this scope.';
    }
    let drag=null;
    svg.addEventListener('pointerdown',e=>{if(e.target.closest?.('.feed-node'))return;drag={x:e.clientX,y:e.clientY,old:{...bounds}};svg.setPointerCapture?.(e.pointerId);});
    svg.addEventListener('pointermove',e=>{if(!drag)return;const w=svg.getBoundingClientRect().width||1000,h=svg.getBoundingClientRect().height||640;
      bounds.x=drag.old.x-(e.clientX-drag.x)*bounds.w/w;bounds.y=drag.old.y-(e.clientY-drag.y)*bounds.h/h;view();});
    const end=()=>{drag=null;};svg.addEventListener('pointerup',end);svg.addEventListener('pointercancel',end);
    svg.addEventListener('wheel',e=>{e.preventDefault();scale(e.deltaY>0?1.15:0.87);},{passive:false});
    return {render,destroy(){container.replaceChildren();},select:(id)=>select(id,false)};
  }
  const api={build,positions,mount};
  if(typeof module!=='undefined'&&module.exports)module.exports=api;
  if(root)root.MuninnFeedGraph=api;
})(typeof window!=='undefined'?window:null);
