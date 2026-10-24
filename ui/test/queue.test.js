'use strict';
const test=require('node:test');const assert=require('node:assert/strict');const Q=require('../queue');
test('queue transitions preserve order, skip unavailable and stop at bounded edges',()=>{const items=[{id:'a',available:true},{id:'b',available:false},{id:'c',available:true}];assert.equal(Q.next(items,0,1),2);assert.equal(Q.next(items,2,1),null);assert.equal(Q.next(items,2,-1),0);assert.equal(Q.next(items,0,-1),null);});
test('ordinary video has no queue transitions',()=>assert.equal(Q.next([],0,1),null));
