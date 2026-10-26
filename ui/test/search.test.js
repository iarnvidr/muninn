'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const S=require('../search.js');
const L=require('../lib.js');

test('search distinguishes saved from disposable cached copies across all qualities',()=>{
  const rows=S.libraryIndex([
    {video_id:'cache',title:'Cache only',state:'cached',status:'complete',protected:false},
    {video_id:'mixed',title:'Saved version',state:'partial',protected:true},
    {video_id:'mixed',state:'cached',status:'complete',protected:false},
    {video_id:'link',title:'Bookmark',state:'link',saved_link:true,protected:false},
    {video_id:'downloaded-link',state:'cached',saved_link:true,status:'complete',protected:false},
    {video_id:'saved-pending',state:'protected',status:'queued',protected:true},
  ]);
  assert.equal(rows.length,5);
  assert.deepEqual(rows.map(v=>[v.id,v.saved,v.complete]),[
    ['cache',false,true],['mixed',true,true],['link',true,false],['downloaded-link',true,true],['saved-pending',true,false],
  ]);
  assert.equal(rows[1].savedMedia,true);
  assert.equal(rows[2].savedMedia,false);
  assert.equal(rows[2].savedLink,true);
});
test('saved channel search excludes incidental browsing but includes groups and disabled configs',()=>{
  const rows=S.savedChannels([
    {channel_id:'browse',name:'Browse',source:'browsing'},
    {channel_id:'sub',name:'Subscribed',source:'subscriptions'},
    {channel_id:'gone',source:'subscriptions',unsubscribed:true},
    {channel_id:'group',name:'Group name',source:'browsing'},
  ],[{channel_id:'disabled',name:'Paused',enabled:false}],[{channel_ids:['group','no-metadata']}]);
  assert.deepEqual(rows.map(v=>v.id),['sub','group','disabled','no-metadata']);
  assert.equal(rows.find(v=>v.id==='group').title,'Group name');
});
test('local matching handles multiple words, accents, URLs and channel names',()=>{
  const rows=[{id:'a',title:'Café videos',channel:'Alice'},{id:'b',title:'Video cafe'},{id:7,title:'Open tab',url:'https://youtube.com/watch?v=local'}];
  assert.deepEqual(S.matches(rows,'CAFE alice').map(v=>v.id),['a']);
  assert.deepEqual(S.matches(rows,'watch local').map(v=>v.id),[7]);
  assert.deepEqual(S.matches(rows,'missing'),[]);
});
test('online categories are explicit and preserve a safely encoded query',()=>{
  const query='cats & sp=wrong # channel';
  const filters={videos:'EgIQAQ==',live:'EgJAAQ==',channels:'EgIQAg=='};
  for(const [category,sp] of Object.entries(filters)) {
    const url=new URL(S.liveUrl(query,category));
    assert.equal(url.origin,'https://www.youtube.com');
    assert.equal(url.pathname,'/results');
    assert.equal(url.searchParams.get('search_query'),query);
    assert.deepEqual(url.searchParams.getAll('sp'),[sp]);
  }
  assert.throws(()=>S.liveUrl(' ', 'videos'));
  assert.throws(()=>S.liveUrl('cats', 'unknown'));
});
test('search shortcut requires the command/control modifier',()=>{
  assert.equal(L.shortcutAction({key:'k'}),null);
  assert.equal(L.shortcutAction({key:'k',metaKey:true}),'search');
  assert.equal(L.shortcutAction({key:'K',ctrlKey:true}),'search');
  assert.equal(L.shortcutAction({key:'k',ctrlKey:true,altKey:true}),null);
});
