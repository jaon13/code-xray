#!/usr/bin/env python3
"""Unit tests for semantic WPF/XAML Style, Template & ResourceDictionary X-Ray."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from xaml_style_xray import scan_xaml_repo  # noqa: E402


class XamlStyleXrayTests(unittest.TestCase):
    def test_detects_order_independent_style_clones_and_based_on(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Themes").mkdir()
            (root / "Views").mkdir()
            (root / "Themes" / "Buttons.xaml").write_text(
                """
<ResourceDictionary xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
                    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <Style x:Key="PrimaryBtn" TargetType="Button">
    <Setter Property="Padding" Value="12,6" />
    <Setter Property="FontSize" Value="13" />
    <Setter Property="FontWeight" Value="SemiBold" />
    <Setter Property="Cursor" Value="Hand" />
  </Style>
</ResourceDictionary>
""".strip(),
                encoding="utf-8",
            )
            (root / "Views" / "MainView.xaml").write_text(
                """
<UserControl xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
             xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <UserControl.Resources>
    <Style x:Key="DuplicateBtn" TargetType="Button">
      <Setter Property="Cursor" Value="Hand" />
      <Setter Property="FontWeight" Value="SemiBold" />
      <Setter Property="FontSize" Value="13" />
      <Setter Property="Padding" Value="12,6" />
    </Style>
    <Style x:Key="ExtendedBtn" TargetType="Button">
      <Setter Property="Padding" Value="12,6" />
      <Setter Property="FontSize" Value="13" />
      <Setter Property="FontWeight" Value="SemiBold" />
      <Setter Property="BorderThickness" Value="1" />
    </Style>
  </UserControl.Resources>
  <StackPanel>
    <Button Style="{StaticResource PrimaryBtn}" />
  </StackPanel>
</UserControl>
""".strip(),
                encoding="utf-8",
            )

            report = scan_xaml_repo(root, min_shared_setters=3, sim_threshold=0.60)
            self.assertEqual(len(report["exactStyleClones"]), 1)
            self.assertGreaterEqual(len(report["basedOnCandidates"]), 1)
            dead_keys = [f for f in report["findings"] if f["id"] == "DeadResource"]
            self.assertTrue(any("DuplicateBtn" in f["message"] for f in dead_keys))

    def test_detects_template_clones_inline_clusters_conflicts_and_hygiene(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "ViewA.xaml").write_text(
                """
<UserControl xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
             xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <UserControl.Resources>
    <SolidColorBrush x:Key="DupBrush" Color="#112233" />
    <SolidColorBrush x:Key="DupBrush" Color="#445566" />
    <Style TargetType="TextBlock"><Setter Property="FontSize" Value="12" /></Style>
    <Style TargetType="TextBlock"><Setter Property="FontSize" Value="14" /></Style>
    <DataTemplate x:Key="ItemTmplA">
      <StackPanel Orientation="Horizontal">
        <TextBlock Text="{Binding Name}" />
      </StackPanel>
    </DataTemplate>
  </UserControl.Resources>
  <StackPanel>
    <Button Margin="4" Padding="8,4" FontSize="12" />
    <Button Margin="4" Padding="8,4" FontSize="12" />
    <Button Margin="4" Padding="8,4" FontSize="12" />
  </StackPanel>
</UserControl>
""".strip(),
                encoding="utf-8",
            )
            (root / "ViewB.xaml").write_text(
                """
<UserControl xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
             xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <UserControl.Resources>
    <DataTemplate x:Key="ItemTmplB">
      <StackPanel Orientation="Horizontal">
        <TextBlock Text="{Binding Title}" />
      </StackPanel>
    </DataTemplate>
  </UserControl.Resources>
</UserControl>
""".strip(),
                encoding="utf-8",
            )

            report = scan_xaml_repo(root)
            ids = {f["id"] for f in report["findings"]}
            self.assertIn("DuplicateResourceKey", ids)
            self.assertIn("ImplicitStyleConflict", ids)
            self.assertIn("MissingFreeze", ids)
            self.assertIn("HardcodedColor", ids)
            self.assertEqual(len(report["templateClones"]), 1)
            self.assertEqual(len(report["inlinePropertyClusters"]), 1)
            self.assertFalse(report["passed"])


if __name__ == "__main__":
    unittest.main()
