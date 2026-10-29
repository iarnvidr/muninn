'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const S=require('../search.js');
const L=require('../lib.js');

test('search distinguishes saved from disposable cached copies across all qualities',()=>{
  const rows=S.libraryIndex([
    {video_id:'cache',title:'Cache only',state:'cached',status:'complete',protected:false},
    {video_id:'mixed',title:'Saved version',state:'partial',protected:true},
    {video_id:'mixed',state:'cached',status:'complete',protected:false,duration_seconds:123},
    {video_id:'link',title:'Bookmark',state:'link',saved_link:true,protected:false},
    {video_id:'downloaded-link',state:'cached',saved_link:true,status:'complete',protected:false},
    {video_id:'saved-pending',state:'protected',status:'queued',protected:true},
  ]);
  assert.equal(rows.length,5);
  assert.deepEqual(rows.map(v=>[v.id,v.saved,v.complete]),[
    ['cache',false,true],['mixed',true,true],['link',true,false],['downloaded-link',true,true],['saved-pending',true,false],
  ]);
  assert.equal(rows[1].savedMedia,true);
  assert.equal(rows[1].duration_seconds,123);
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
test('arrows and hjkl map to navigation without stealing text, modifiers or IME input',()=>{
  for(const [key,direction] of Object.entries({h:'left',ArrowLeft:'left',j:'down',ArrowDown:'down',k:'up',ArrowUp:'up',l:'right',ArrowRight:'right'})) {
    assert.equal(S.navigationDirection({key},false),direction);
    assert.equal(S.navigationDirection({key},true),null);
    for(const modifier of ['ctrlKey','metaKey','altKey','shiftKey','isComposing'])assert.equal(S.navigationDirection({key,[modifier]:true},false),null);
  }
});
test('result navigation crosses types and online/local bands, clamping unequal columns',()=>{
  const bands=[[4,2,3],[3,0,2]];
  assert.deepEqual(S.moveCursor(bands,{band:0,column:0,row:3},'right'),{band:0,column:1,row:1});
  assert.deepEqual(S.moveCursor(bands,{band:0,column:0,row:3},'down'),{band:1,column:0,row:0});
  assert.deepEqual(S.moveCursor(bands,{band:1,column:2,row:0},'up'),{band:0,column:2,row:2});
  assert.deepEqual(S.moveCursor(bands,{band:1,column:0,row:1},'right'),{band:1,column:2,row:1});
  assert.equal(S.moveCursor(bands,{band:0,column:1,row:0},'up'),null);
  assert.deepEqual(S.moveCursor(bands,{band:1,column:2,row:1},'down'),{band:1,column:2,row:1});
});
test('search shortcut requires the command/control modifier',()=>{
  assert.equal(L.shortcutAction({key:'k'}),null);
  assert.equal(L.shortcutAction({key:'k',metaKey:true}),'search');
  assert.equal(L.shortcutAction({key:'K',ctrlKey:true}),'search');
  assert.equal(L.shortcutAction({key:'k',ctrlKey:true,altKey:true}),null);
});
