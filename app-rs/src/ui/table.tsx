/**
 * 轻量 Table（AntD Table 替代，覆盖本项目实际用到的子集）：
 * 列定义 + 数据源 + 行键 + 空态 + 行事件；分页/排序/虚拟化按需后补。
 *
 * 设计取舍：不自建通用表格状态机（那是 AntD 的核心复杂度）——本项目
 * 表格多为「列表展示 + 行操作」，列宽/渲染/对齐由 columns 显式声明即可。
 */
import type { ReactNode } from "react";
import { cx } from "./index";

export interface Column<T> {
  title: ReactNode;
  /** 取值字段名；或自定义 render(row) */
  dataIndex?: keyof T;
  width?: number;
  align?: "left" | "right" | "center";
  render?: (value: T[keyof T], row: T) => ReactNode;
}

export function Table<T>({
  columns,
  dataSource,
  rowKey,
  empty,
  loading,
  onRowClick,
  rowClassName,
  className,
}: {
  columns: Column<T>[];
  dataSource: T[];
  rowKey: (row: T) => string | number;
  empty?: ReactNode;
  loading?: boolean;
  onRowClick?: (row: T) => void;
  rowClassName?: (row: T) => string;
  className?: string;
}) {
  if (loading) {
    return <div className="py-6 text-center text-sm text-sk-sub">加载中…</div>;
  }
  if (dataSource.length === 0 && empty) {
    return <>{empty}</>;
  }
  return (
    <div className={cx("overflow-x-auto", className)}>
      <table className="w-full border-collapse">
        <thead>
          <tr>
            {columns.map((c, i) => (
              <th
                key={i}
                style={c.width ? { width: c.width } : undefined}
                className={cx(
                  "border-b border-sk-border px-2.5 pb-2 pt-1 text-left align-bottom text-xs font-medium text-sk-sub",
                  c.align === "right" && "text-right",
                  c.align === "center" && "text-center",
                )}
              >
                {c.title}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {dataSource.map((row) => (
            <tr
              key={rowKey(row)}
              onClick={onRowClick ? () => onRowClick(row) : undefined}
              className={cx(
                "transition-colors duration-150 hover:bg-sk-hover",
                onRowClick && "cursor-pointer",
                rowClassName?.(row),
              )}
            >
              {columns.map((c, i) => (
                <td
                  key={i}
                  style={c.width ? { width: c.width } : undefined}
                  className={cx(
                    "border-b border-sk-border/60 px-2.5 py-2 align-middle text-sm text-sk-text",
                    c.align === "right" && "text-right",
                    c.align === "center" && "text-center",
                  )}
                >
                  {c.render
                    ? c.render(row[c.dataIndex!], row)
                    : c.dataIndex
                      ? (() => {
                          const v = row[c.dataIndex];
                          return v === null || v === undefined ? "" : String(v);
                        })()
                      : null}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
