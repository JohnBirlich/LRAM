import {test} from 'node:test';
import assert from 'node:assert/strict';
import {maskAccount,maskGroup} from '../src/features/accounts/privacyData.ts';

test('privacy is a display projection, never a mutation of saved account data or IDs',()=>{
 const a=Object.freeze({id:7,userId:123456,username:'private_user',displayName:'Private Display',alias:'My Main',notes:'Private notes',color:'#123456',groupIds:[12],status:'valid',fullQuality:false});
 const hidden=maskAccount(a);
 for(const k of ['alias','displayName','notes']) assert.equal(hidden[k],'Hidden');
 assert.equal(hidden.username,'hidden'); assert.equal(hidden.color,null);
 assert.equal(hidden.id,a.id);assert.equal(hidden.userId,a.userId);assert.equal(hidden.status,'valid');
 assert.equal(a.username,'private_user');assert.equal(a.notes,'Private notes');assert.notEqual(a,hidden);
});
test('groups are hidden without changing memberships or saved group names',()=>{
 const g=Object.freeze({id:12,name:'Private group',color:'#123456',sortOrder:3});
 assert.deepEqual(maskGroup(g),{id:12,name:'Hidden',color:'#808080',sortOrder:3});assert.equal(g.name,'Private group');
});
