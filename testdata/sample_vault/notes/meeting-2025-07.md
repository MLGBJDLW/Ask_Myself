# 2025-07 会议纪要 | July 2025 Meeting Notes

## 议题 Agenda

### 1. 项目进度回顾 Project Progress Review

本月完成了以下关键里程碑：
- 完成了核心搜索引擎的 FTS5 集成 (Completed FTS5 integration for core search engine)
- 实现了文档解析器的 Markdown 支持 (Implemented Markdown support in document parser)
- 优化了数据库查询性能，平均响应时间降低 40% (Optimized DB query performance, avg response time reduced by 40%)

### 2. 技术讨论 Technical Discussion

关于向量搜索的实现方案，团队讨论了以下选项：

1. **sqlite-vec** — 轻量级，与现有 SQLite 架构无缝集成
2. **FAISS** — 成熟但需要额外的 C++ 依赖
3. **自研方案** — 灵活性最高但开发成本大

```sql
-- 示例：FTS5 搜索查询
SELECT d.title, c.content, rank
FROM chunks_fts AS f
JOIN chunks AS c ON c.id = f.rowid
JOIN documents AS d ON d.id = c.document_id
WHERE chunks_fts MATCH '搜索引擎 OR search engine'
ORDER BY rank
LIMIT 10;
```

### 3. 行动项 Action Items

| 负责人 Owner | 任务 Task | 截止日期 Deadline |
|---|---|---|
| 张三 Zhang San | 完成 sqlite-vec 原型 Complete sqlite-vec prototype | 2025-07-20 |
| 李四 Li Si | 编写解析器单元测试 Write parser unit tests | 2025-07-15 |
| 王五 Wang Wu | 性能基准测试报告 Performance benchmark report | 2025-07-25 |

### 4. 下次会议 Next Meeting

- 日期：2025-07-14 (周一)
- 时间：14:00 CST
- 议题：sqlite-vec 原型演示与性能评估
