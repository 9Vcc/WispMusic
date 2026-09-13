const createOption = require('../util/option.js')

module.exports = async (query, request, externals) => {
  const imgFile = query.imgFile
  const data = {
    bucket: 'yyimgs',
    ext: 'jpg',
    filename: imgFile.name,
    local: false,
    nos_product: 0,
    return_body: `{"code":200,"size":"$(ObjectSize)"}`,
    type: 'other',
  }
  const res = await request(
    `/api/nos/token/alloc`,
    data,
    createOption(query, 'weapi'),
  )

  await externals.fetchImpl(
    `https://nosup-hz1.127.net/yyimgs/${res.body.result.objectKey}?offset=0&complete=true&version=1.0`,
    {
      method: 'post',
      headers: {
        'x-nos-token': res.body.result.token,
        'Content-Type': imgFile.type || 'image/jpeg',
      },
      data: await imgFile.arrayBuffer(),
    },
  )

  return {
    url_pre: 'https://p1.music.126.net/' + res.body.result.objectKey,
    imgId: res.body.result.docId,
  }
}
