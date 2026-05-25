# Topview 影片智能體 V2 (Video Agent V2) · AutoCLI adapter 建置交接 spec

> **Status**: 🟡 spec only · NEXT SESSION 建 **Option B 全流程自動化 adapter**(Sunny lock · context 滿故本 session 只記錄)
> **緣起**: Sunny 指派「把 Topview 影片智能體 V2 功能加進 AutoCLI」· 來源教程 YouTube `HIbY8TXVJmU`(小發 · "AI 影片真能拍成一整部了!不限時長+可控分鏡")
> **字幕全文**: `~/Documents/nProjecs/hermesDr/ref-docs/AI 影片真能拍成一整部了_! 不限時長 + 可控分鏡，還能像導演一樣安排每一幕-Topview AI 教程.txt`(520 條 SRT · 已詳讀)
> **UI 截圖**: `frames/01-07*.jpg`(本資料夾 · 從教程影片關鍵時間點抽)
> **Sunny locks**: ① 下 session 做 Option B(全流程自動化)② V2 在 6宮格 Layer 6 的定位**先不定**,先把工具建好 ③ 花積分前 checkpoint
> **作者**: Neo · S367 2026-05-26

---

## §1 · 功能總覽

影片智能體 V2 = Topview 新功能 · **不限時長**的 storyboard-driven 影片生成 · 比現有 `submit-omni-direct`(per-clip ≤15s)強在「Agent 自動拆段 + 不限時長 + 可延展 + 可匯出整片」。

**banner (frame 01)**: `Seedance 2.0 720P $0.1/S | Seedance 2.0 & GPT-Image-2 · 365 UNLIMITED`
→ ⚠️ **365 UNLIMITED 待驗**:若 Ultra 方案下 Seedance 2.0 在 V2 真不耗積分,則 V2 出片**免費**(vs submit-omni-direct 耗 Ultra 積分 0.5cr/s@480p)= Layer 6 大利多。但 frame 02 sparkle 顯示「37.5」· frame 03/04 顯示「生成 +5」「+0.8」→ 仍有數字,需 live recon 確認到底耗不耗。

---

## §2 · 三階段流程(adapter 要自動化的全鏈 = Option B)

### Stage 1 · 提交頁(frame 01 → 02)
- **入口**: Topview 首頁(點左上 logo)→ tab bar「**影片智能體 V2**(下拉)| AI 影片 | AI 圖像」· V2 是預設 tab。⚠️ 可能有直接 URL,recon 要找。
- **參考圖**: 上傳 **1-5 張**(圖或影片)· 描述文字「上傳 1-5 張參考圖或影片並用 @ 引用」· @Image1/@Image2/@Video1 引用(= **跟 submit-omni-direct 同 @ 機制**,可重用 P3 chip-binding 邏輯)。
- **prompt**: contenteditable · 例「Generate a video based on the storyboard in @Image1」+ 故事描述。
- **設定控制列**(底部): `Seedance 2.0`(model 下拉)· `16:9/9:16`(ratio)· `720p/1080p`(res)· `15s`(duration 下拉 → 自動/自訂 + 15s/30s/45s/60s presets)· @ 鈕 · sparkle 數字 · submit 箭頭(↑)。
- 點 submit → 進 Stage 2。

### Stage 2 · Agent 規劃頁(frame 03)
- 標題列: shot 名 / `回饋` / `匯出`(右上)。
- 左欄: 「Agent · {shot}」+「正在為您規劃...的 30 秒短影片方案...」+ **拆段草稿**:
  - 場景 1 (15S): 風格參考 @Image1 · [0-5s] ... [5-10s] ... [10-15s] ... 構圖:...
  - 場景 2 (15S): 延續場景一風格 · [0-5s] ...
  - 底部輸入「繼續優化這份草稿...」+ 風格:Auto + 送出箭頭(可對話式改草稿)。
- 右欄: 「時間軸為空 · 請在下方輸入框描述一個場景,生成你的第一個片段」+ 素材(N)/已生成 tabs + Image thumbnails + 「描述你的場景。使用 @ 引用已上傳素材」輸入 + 控制列(Seedance 2.0 · ratio · 5s · res · 故事板 +0.8 · **生成 +5**)。

### Stage 3 · 時間軸編輯器(frame 04 → 05)
- 影片預覽(▶)+ 時間軸 clips(Clip 1, Clip 2...)+ `15.0s / 30.0s` 進度 + zoom。
- 右欄: 「Clip N」+ 該段描述(風格參考 @Image1 · [0-5s]...)。
- **延展**: clips 尾端「→|」icon · 或底部輸入「延展 Clip N」chip → 「從 Clip N 延展...」→ 不限時長(frame 05: Clip 4「從 Clip 1 延展」· 27.8s/30.0s)。
- **匯出**(右上): 匯出完整影片 OR 匯出所有片段(字幕 #468-473)。

---

## §3 · Adapter 設計(Option B · 可重用既有資產)

**可重用 `submit-omni-direct.yaml` 的部分**:
- P0 prompt 解析 @Image markers → prose segments
- P1.6/P1.7 dropdown set(resolution/duration)pattern(locate btn → click → option → verify)
- P2 `upload-file-trusted`(input[type=file] · setFileInputFiles + click override 防 picker leak · S329 fix)
- P3 @ chip binding(execCommand insertText prose + CDP `Input.dispatchKeyEvent @` + Enter/ArrowDown 選 @ImageN)— **這是最值錢可重用的**
- P4 submit + toast 偵測(任務提交成功)
- download-mp4.yaml(匯出後抓 mp4 · 但 V2 匯出機制可能不同,recon 確認)

**V2 新增需 recon 的**:
1. **入口導航**: V2 page URL(或 首頁→tab 點擊序)· tab「影片智能體 V2」selector。
2. **參考圖上傳**: 1-5 張的 upload box selector(frame 02 「+參考」box)。
3. **設定下拉**: model/ratio/res/duration 的 button selector + option selector(V2 page 的,跟 board 面板不同 DOM)。
4. **Stage 2 偵測**: 提交後如何知道進了 Agent 規劃頁(URL 變? 「正在為您規劃」字串?)· 草稿 ready 偵測。
5. **Stage 2→3 觸發生成**: 「生成 +N」鈕 selector · per-segment 生成。
6. **延展**: 「→|」extend icon selector + 延展輸入。
7. **匯出**: 匯出鈕 + 「完整影片/所有片段」選項 + 下載落地(配 download-mp4 信任 click · 注意 gh#58 daemon 閃斷)。
8. **成本確認**: 生成前讀 sparkle/積分數字 · 驗 365 UNLIMITED 是否真免費。

**Option B 建議分 adapter / phase**:
- `recon-video-agent-v2.yaml`: navigate + dump V2 page DOM inventory(button/input/contenteditable/dropdown · 仿 recon.yaml)→ 拿真實 selector。
- `submit-video-agent-v2.yaml`: Stage 1 提交(upload + prompt@ + 設定 + submit)→ 落 Agent 規劃頁。
- `generate-segments-v2.yaml` (or 併入): Stage 2→3 逐段生成 + poll。
- `export-video-v2.yaml`: Stage 3 匯出 + 下載。
- 或一支大 pipeline 串全鏈(風險高 · 建議分段 + 各自可 resume)。

---

## §4 · 建置順序(NEXT SESSION)
1. daemon up + Chrome 登入 Topview(`curl 127.0.0.1:19925/status` extensionConnected:true)。
2. 跑 `recon-video-agent-v2.yaml` → 拿 Stage 1 真實 selector。
3. 建 `submit-video-agent-v2.yaml`(重用 submit-omni P0/P2/P3/P1.6-7)· 用一張 koDr-v2 6宮格分鏡圖當 ref 測提交(⚠️ 生成前 checkpoint Sunny · 驗 365 UNLIMITED)。
4. recon Stage 2/3(Agent 頁 + 時間軸)→ 建 generate + export。
5. 全鏈 smoke(餵 s0 或 S1 6宮格 → V2 出整片 → 匯出 mp4 → analyze-seedance 驗)。
6. 驗證 6宮格 fit(face lock / 不出鬼片 / honor 分鏡)→ 回報 Sunny 決定 V2 定位(主路 vs 輔助)。

---

## §5 · 紅線 / 注意
- **Layer 6 = AutoCLI ONLY · NEVER API**(`feedback_topview_layer6_autocli_only_never_api`)· V2 也走 AutoCLI 網頁版。
- **花積分前 checkpoint Sunny**(本 adapter 第一次真生成前)· 先驗 365 UNLIMITED。
- **download 注意 gh#58**(daemon 閃斷 · click 派發但檔案不落地 · 必要時 Sunny 手動下載)。
- **靜默 beat**: V2 prompt 要加「角色全程不說話,只用眼神/表情/肢體」防 AI 加戲(字幕 #239-243 · = 我們既有禁字幕/靜默紅線)。
- **6宮格 fit 未驗**: V2 Agent 自動拆段,可能不嚴格 honor 我們 per-panel keyframe · 需實測(Sunny 定位先不定)。

---

## §6 · Cross-ref
- 字幕: `hermesDr/ref-docs/AI 影片真能拍成一整部了_!...Topview AI 教程.txt`
- 截圖: 本資料夾 `frames/01-07*.jpg`
- 既有 adapter 範本: `adapters/topview/submit-omni-direct.yaml`(P0-P4 全鏈)· `recon.yaml`(DOM dump 範式)· `download-mp4.yaml`
- 既有 Layer 6 紀律: `feedback_autocli_topview_chip_binding` · `feedback_topview_layer6_autocli_only_never_api`
- storyboard-spec: `hermesDr/docs-drafts/koDr-v2/storyboard-spec.md` §6 已提及此 V2「待驗」

**Last updated**: S367 2026-05-26 · Neo · spec only · NEXT = build Option B
